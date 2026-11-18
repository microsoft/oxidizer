// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Keyboard and mouse navigation over mimalloc heap lifetimes and page bins.

use std::borrow::Cow;

use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::Paragraph;
use seismograph_rallocator::mimalloc::{CLASS_COUNT, State};
use seismograph_rallocator::snapshot::Header;

use super::help::Context;
use super::mouse::{ListTarget, MouseRows};
use super::native_ui::{detail, frame, list};
use crate::mimalloc_view;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Depth {
    #[default]
    Root,
    Classes,
    Detail,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Navigation {
    pub(super) root: usize,
    pub(super) selected_owner: Option<u64>,
    pub(super) class: usize,
    pub(super) scroll: usize,
    pub(super) depth: Depth,
}

impl Navigation {
    pub(super) fn reconcile(&mut self, state: &State) {
        if let Some(id) = self.selected_owner {
            if let Some(index) = state.owners.iter().position(|owner| owner.id == id) {
                self.root = index + 1;
            } else {
                *self = Self::default();
            }
        } else {
            self.root = self.root.min(state.owners.len());
        }
        self.remember_owner(state);
    }

    fn remember_owner(&mut self, state: &State) {
        self.selected_owner = self
            .root
            .checked_sub(1)
            .and_then(|index| state.owners.get(index))
            .map(|owner| owner.id);
    }

    pub(super) fn key(&mut self, key: KeyCode, state: Option<&State>) -> bool {
        if let Some(state) = state {
            self.reconcile(state);
        }
        if matches!(key, KeyCode::Esc | KeyCode::Backspace) {
            self.depth = match self.depth {
                Depth::Root => return false,
                Depth::Detail if self.root != 0 => Depth::Classes,
                _ => Depth::Root,
            };
            self.scroll = 0;
            return true;
        }
        if key == KeyCode::Enter {
            self.depth = match self.depth {
                Depth::Root if self.root != 0 => Depth::Classes,
                _ => Depth::Detail,
            };
            self.scroll = 0;
            return true;
        }
        if self.depth == Depth::Detail {
            self.scroll = match key {
                KeyCode::Up => self.scroll.saturating_sub(1),
                KeyCode::Down => self.scroll.saturating_add(1),
                KeyCode::PageUp => self.scroll.saturating_sub(5),
                KeyCode::PageDown => self.scroll.saturating_add(5),
                KeyCode::Home => 0,
                KeyCode::End => usize::MAX,
                _ => return false,
            };
            return true;
        }
        let (selected, count) = if self.depth == Depth::Root {
            (&mut self.root, state.map_or(1, |state| state.owners.len() + 1))
        } else {
            (&mut self.class, CLASS_COUNT)
        };
        *selected = match key {
            KeyCode::Up => selected.saturating_sub(1),
            KeyCode::Down => selected.saturating_add(1).min(count - 1),
            KeyCode::PageUp => selected.saturating_sub(5),
            KeyCode::PageDown => selected.saturating_add(5).min(count - 1),
            KeyCode::Home => 0,
            KeyCode::End => count - 1,
            _ => return false,
        };
        self.scroll = 0;
        if self.depth == Depth::Root {
            self.class = 0;
            if let Some(state) = state {
                self.remember_owner(state);
            }
        }
        true
    }

    pub(super) fn click(&mut self, target: ListTarget, index: usize, state: &State) {
        match target {
            ListTarget::HeapBuckets => {
                self.root = index.min(state.owners.len());
                self.selected_owner = None;
                self.depth = Depth::Root;
                self.class = 0;
            }
            ListTarget::NativeClasses => {
                self.class = index.min(CLASS_COUNT - 1);
                self.depth = Depth::Classes;
            }
            _ => return,
        }
        self.scroll = 0;
        self.reconcile(state);
    }

    pub(super) fn focus_list(&mut self, target: ListTarget) {
        self.depth = match target {
            ListTarget::HeapBuckets => Depth::Root,
            ListTarget::NativeClasses => Depth::Classes,
            _ => self.depth,
        };
    }

    pub(super) const fn help(self) -> Context {
        if self.root == 0 {
            Context::MimallocGlobal
        } else if matches!(self.depth, Depth::Classes | Depth::Detail) {
            Context::MimallocClasses
        } else {
            Context::MimallocOwner
        }
    }
}

pub(super) fn draw(
    terminal: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    header: &Header,
    state: &State,
    mut nav: Navigation,
    unavailable: Option<&str>,
) {
    nav.reconcile(state);
    let summary = mimalloc_view::overview(header, state);
    let summary_height = area.height.saturating_sub(5).min(6);
    let [top, body] = Layout::vertical([Constraint::Length(summary_height), Constraint::Min(1)]).areas(area);
    terminal.render_widget(
        Paragraph::new(summary.join("\n")).block(frame(" Mimalloc state · b switches backend when both exist ".into(), false)),
        top,
    );
    let compact = area.width < 90;
    let owner_width = if compact && nav.depth != Depth::Root {
        0
    } else {
        area.width.min(32)
    };
    let [owners, content] = Layout::horizontal([Constraint::Length(owner_width), Constraint::Min(1)]).areas(body);
    list(
        terminal,
        mouse,
        owners,
        (state.owners.len() + 1, |index| {
            if index == 0 {
                Cow::Borrowed("Global backend")
            } else {
                Cow::Owned(mimalloc_view::owner_label(header, &state.owners[index - 1]))
            }
        }),
        nav.root,
        ListTarget::HeapBuckets,
        frame(" Mimalloc · Enter details ".into(), nav.depth == Depth::Root),
    );
    if let Some(error) = unavailable {
        detail(
            terminal,
            mouse,
            content,
            " Source / capture error ".into(),
            &[error.into()],
            nav.scroll,
            nav.depth != Depth::Root,
        );
        return;
    }
    let Some(owner) = nav.root.checked_sub(1).and_then(|index| state.owners.get(index)) else {
        let mut lines = summary;
        lines.extend(mimalloc_view::global_lines(state));
        detail(
            terminal,
            mouse,
            content,
            " Mimalloc global backend · Enter to scroll ".into(),
            &lines,
            nav.scroll,
            nav.depth == Depth::Detail,
        );
        return;
    };
    let class_width = if compact && nav.depth == Depth::Detail {
        0
    } else {
        content.width.min(24)
    };
    let [classes, content] = Layout::horizontal([Constraint::Length(class_width), Constraint::Min(1)]).areas(content);
    list(
        terminal,
        mouse,
        classes,
        (CLASS_COUNT, |index| Cow::Owned(mimalloc_view::class_label(owner, index))),
        nav.class,
        ListTarget::NativeClasses,
        frame(" Classes · Enter details ".into(), nav.depth == Depth::Classes),
    );
    detail(
        terminal,
        mouse,
        content,
        format!(" Heap #{} · class {} · F1 ", owner.id, nav.class),
        &mimalloc_view::class_lines(header, owner, nav.class),
        nav.scroll,
        nav.depth == Depth::Detail,
    );
}
