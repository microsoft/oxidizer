// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared table presentation and mouse-target layout.

use std::cmp::Ordering;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders};

use super::mouse::{ListTarget, MouseRows};

pub(super) fn sort_heading(label: &str, selected: bool, descending: bool) -> String {
    if selected {
        format!("{label}{}", if descending { "▼" } else { "▲" })
    } else {
        label.into()
    }
}

pub(super) fn directed(ordering: Ordering, descending: bool) -> Ordering {
    if descending { ordering.reverse() } else { ordering }
}

pub(super) fn sort_headers<T: Copy + Eq>(headers: Vec<&str>, sorts: &[T], selected: T, descending: bool) -> Vec<String> {
    headers
        .into_iter()
        .zip(sorts)
        .map(|(header, sort)| sort_heading(header, *sort == selected, descending))
        .collect()
}

pub(super) fn register_headers(mouse: &MouseRows, area: Rect, widths: &[Constraint], targets: &[ListTarget]) {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    if inner.is_empty() {
        return;
    }
    for (column, target) in Layout::horizontal(widths.iter().copied())
        .spacing(1)
        .split(inner)
        .iter()
        .zip(targets)
    {
        mouse.register_item(Rect::new(column.x, inner.y, column.width, 1), *target, 0);
    }
}

pub(super) fn selection_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().bg(Color::DarkGray)
    }
}

pub(super) fn focused_block(title: impl Into<Line<'static>>, focused: bool) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(if focused { Color::Cyan } else { Color::DarkGray }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::monitor::app::HotspotSort;

    #[test]
    fn mixed_constraints_create_non_overlapping_header_targets() {
        let mouse = MouseRows::default();
        let area = Rect::new(0, 0, 40, 5);
        mouse.begin(area);
        register_headers(
            &mouse,
            area,
            &[Constraint::Length(8), Constraint::Min(10), Constraint::Length(6)],
            &[
                ListTarget::HotspotHeader(HotspotSort::Severity),
                ListTarget::HotspotHeader(HotspotSort::Finding),
                ListTarget::HotspotHeader(HotspotSort::Location),
            ],
        );

        assert_eq!(mouse.at(area, 1, 1), Some((ListTarget::HotspotHeader(HotspotSort::Severity), 0)));
        assert_eq!(mouse.at(area, 10, 1), Some((ListTarget::HotspotHeader(HotspotSort::Finding), 0)));
        assert_eq!(mouse.at(area, 33, 1), Some((ListTarget::HotspotHeader(HotspotSort::Location), 0)));
    }

    #[test]
    fn sort_headings_show_only_the_selected_direction() {
        assert_eq!(
            sort_headers(
                vec!["Severity", "Finding"],
                &[HotspotSort::Severity, HotspotSort::Finding],
                HotspotSort::Finding,
                true,
            ),
            ["Severity", "Finding▼"]
        );
    }
}
