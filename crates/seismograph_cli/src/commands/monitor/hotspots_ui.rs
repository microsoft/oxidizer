// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Findings and copy-ready agent instructions for the Hotspots tab.

use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap};

use super::super::app::{HotspotSort, HotspotViewState};
use super::super::data::CapturedSnapshot;
use super::super::hotspot_analysis::{Finding, Report, Severity};
use super::super::mouse::{ListTarget, MouseRows};
use super::{draw_empty_panel_with_message, key_span};

pub(super) fn draw(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    [findings_area, details_area]: [Rect; 2],
    snapshot: Option<&CapturedSnapshot>,
    unavailable: Option<&str>,
    view: HotspotViewState,
) {
    let Some(snapshot) = snapshot else {
        draw_empty_panel_with_message(frame, findings_area, " Hotspots ", unavailable);
        draw_empty_panel_with_message(frame, details_area, " Finding Details ", unavailable);
        return;
    };
    let findings = sorted_findings(&snapshot.hotspots, view);
    draw_findings(frame, mouse, findings_area, &snapshot.hotspots, &findings, view);
    let selected = view.selected.min(findings.len().saturating_sub(1));
    draw_details(
        frame,
        details_area,
        findings.get(selected).copied(),
        snapshot.hotspots.capture_warning.as_deref(),
    );
}

fn sorted_findings(report: &Report, view: HotspotViewState) -> Vec<&Finding> {
    let mut findings = report.findings.iter().enumerate().collect::<Vec<_>>();
    findings.sort_unstable_by(|(left_index, left), (right_index, right)| {
        let ordering = match view.sort {
            HotspotSort::Severity => left.severity.cmp(&right.severity),
            HotspotSort::Category => left.category.label().cmp(right.category.label()),
            HotspotSort::Finding => left.title.cmp(&right.title),
            HotspotSort::Location => left.location.cmp(&right.location),
        };
        let ordering = if view.descending { ordering.reverse() } else { ordering };
        ordering
            .then_with(|| right.severity.cmp(&left.severity))
            .then_with(|| left_index.cmp(right_index))
    });
    findings.into_iter().map(|(_, finding)| finding).collect()
}

fn draw_findings(
    frame: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    report: &Report,
    findings: &[&Finding],
    view: HotspotViewState,
) {
    if area.is_empty() {
        return;
    }
    let selected = view.selected.min(findings.len().saturating_sub(1));
    let visible = usize::from(area.height.saturating_sub(3));
    let first = selected.saturating_sub(visible.saturating_sub(1));
    let (headers, sorts, widths) = columns(area.width);
    let rows = findings.iter().skip(first).take(visible).map(|finding| {
        let values = if area.width >= 76 {
            vec![
                finding.severity.label().to_owned(),
                finding.category.label().to_owned(),
                finding.title.clone(),
                finding.location.clone(),
            ]
        } else if area.width >= 52 {
            vec![
                finding.severity.label().to_owned(),
                finding.category.label().to_owned(),
                finding.title.clone(),
            ]
        } else {
            vec![finding.severity.label().to_owned(), finding.title.clone()]
        };
        Row::new(values.into_iter().enumerate().map(|(index, value)| {
            let cell = Cell::from(value);
            if index == 0 {
                cell.style(severity_style(finding.severity))
            } else {
                cell
            }
        }))
    });
    let title = Line::from(vec![
        Span::raw(format!(" Hotspots · {} findings · ", findings.len())),
        key_span("↑/↓"),
        Span::raw(" select · "),
        key_span("[/]"),
        Span::raw(" sort · "),
        key_span("r"),
        Span::raw(" reverse "),
    ]);
    let empty = findings
        .is_empty()
        .then_some(Row::new(vec!["No actionable hotspots met the evidence thresholds.".to_owned()]));
    let rows = rows.chain(empty);
    let table = Table::new(rows, widths.clone())
        .header(Row::new(sort_headers(headers, &sorts, view)).style(Style::default().add_modifier(Modifier::BOLD)))
        .column_spacing(1)
        .row_highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
        .block(
            Block::default()
                .title(title)
                .title_bottom(if report.capture_warning.is_some() {
                    " Capture loss limits evidence; see Finding Details. "
                } else {
                    " Retained evidence only; absence does not prove no issue. "
                })
                .borders(Borders::ALL),
        );
    let mut state = TableState::default().with_selected((!findings.is_empty()).then_some(selected.saturating_sub(first)));
    frame.render_stateful_widget(table, area, &mut state);
    mouse.register(
        area,
        1,
        first.saturating_add(state.offset()),
        findings.len(),
        ListTarget::HotspotFindings,
    );
    register_sort_headers(mouse, area, &widths, &sorts);
}

fn columns(width: u16) -> (Vec<&'static str>, Vec<HotspotSort>, Vec<Constraint>) {
    if width >= 76 {
        (
            vec!["Severity", "Category", "Finding", "Location"],
            vec![
                HotspotSort::Severity,
                HotspotSort::Category,
                HotspotSort::Finding,
                HotspotSort::Location,
            ],
            vec![
                Constraint::Length(9),
                Constraint::Length(16),
                Constraint::Length(28),
                Constraint::Min(18),
            ],
        )
    } else if width >= 52 {
        (
            vec!["Severity", "Category", "Finding"],
            vec![HotspotSort::Severity, HotspotSort::Category, HotspotSort::Finding],
            vec![Constraint::Length(9), Constraint::Length(16), Constraint::Min(20)],
        )
    } else {
        (
            vec!["Severity", "Finding"],
            vec![HotspotSort::Severity, HotspotSort::Finding],
            vec![Constraint::Length(9), Constraint::Min(16)],
        )
    }
}

fn sort_headers(headers: Vec<&str>, sorts: &[HotspotSort], view: HotspotViewState) -> Vec<String> {
    headers
        .into_iter()
        .zip(sorts)
        .map(|(header, sort)| {
            if *sort == view.sort {
                format!("{header}{}", if view.descending { "▼" } else { "▲" })
            } else {
                header.into()
            }
        })
        .collect()
}

fn register_sort_headers(mouse: &MouseRows, area: Rect, widths: &[Constraint], sorts: &[HotspotSort]) {
    let inner = Block::default().borders(Borders::ALL).inner(area);
    if inner.is_empty() {
        return;
    }
    let spacing = u16::try_from(widths.len().saturating_sub(1)).unwrap_or(u16::MAX);
    let fixed = widths.iter().fold(0_u16, |total, width| {
        total.saturating_add(match width {
            Constraint::Length(width) => *width,
            _ => 0,
        })
    });
    let flexible = inner.width.saturating_sub(fixed).saturating_sub(spacing);
    let mut x = inner.x;
    for (width, sort) in widths.iter().zip(sorts) {
        let width = match width {
            Constraint::Length(width) => *width,
            Constraint::Min(minimum) => flexible.max(*minimum),
            _ => 0,
        }
        .min(inner.right().saturating_sub(x));
        mouse.register_item(Rect::new(x, inner.y, width, 1), ListTarget::HotspotHeader(*sort), 0);
        x = x.saturating_add(width).saturating_add(1);
    }
}

fn draw_details(frame: &mut ratatui::Frame<'_>, area: Rect, finding: Option<&Finding>, capture_warning: Option<&str>) {
    let Some(finding) = finding else {
        frame.render_widget(
            Paragraph::new(
                "No actionable hotspot met the current evidence thresholds.\n\nThis analyzer omits weak signals that need deeper investigation.",
            )
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().title(" Finding Details ").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
            area,
        );
        return;
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                finding.severity.label(),
                severity_style(finding.severity).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" · {}", finding.category.label())),
        ]),
        Line::from(Span::styled(&finding.title, Style::default().add_modifier(Modifier::BOLD))),
        Line::from(format!("Location: {}", finding.location)),
        Line::from(format!("Evidence: {}", finding.evidence)),
        Line::from(""),
        Line::from(Span::styled("Why it matters", Style::default().add_modifier(Modifier::BOLD))),
        Line::from(finding.explanation.as_str()),
        Line::from(""),
        Line::from(Span::styled(
            "Agent instruction",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
        Line::from(finding.agent_instruction.as_str()),
    ];
    if let Some(warning) = capture_warning {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled(
                "Evidence limit",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )),
            Line::from(warning),
        ]);
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().title(" Finding Details ").borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn severity_style(severity: Severity) -> Style {
    Style::default().fg(match severity {
        Severity::High => Color::LightRed,
        Severity::Medium => Color::Yellow,
    })
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::commands::monitor::hotspot_analysis::Category;

    fn finding(severity: Severity, category: Category, title: &str) -> Finding {
        Finding {
            severity,
            category,
            title: title.into(),
            evidence: "evidence".into(),
            location: "src/lib.rs:1:1".into(),
            explanation: "explanation".into(),
            agent_instruction: "instruction".into(),
            score: 1,
        }
    }

    #[test]
    fn sorting_keeps_high_severity_findings_first_by_default() {
        let report = Report {
            findings: vec![
                finding(Severity::Medium, Category::Cache, "cache"),
                finding(Severity::High, Category::Allocation, "allocation"),
            ],
            capture_warning: None,
        };

        let sorted = sorted_findings(
            &report,
            HotspotViewState {
                selected: 0,
                sort: HotspotSort::Severity,
                descending: true,
            },
        );

        assert_eq!(sorted[0].title, "allocation");
    }

    #[test]
    fn findings_and_agent_instruction_render_with_sort_targets() {
        let report = Report {
            findings: vec![finding(Severity::High, Category::Allocation, "large allocation")],
            capture_warning: Some("10.0% overwritten".into()),
        };
        let view = HotspotViewState {
            selected: 0,
            sort: HotspotSort::Severity,
            descending: true,
        };
        let findings = sorted_findings(&report, view);
        let mouse = MouseRows::default();
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal
            .draw(|frame| {
                mouse.begin(frame.area());
                let [left, right] =
                    ratatui::layout::Layout::horizontal([Constraint::Percentage(45), Constraint::Min(1)]).areas(frame.area());
                draw_findings(frame, &mouse, left, &report, &findings, view);
                draw_details(frame, right, findings.first().copied(), report.capture_warning.as_deref());
            })
            .unwrap();
        let output = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();

        assert!(output.contains("Agent instruction"));
        assert!(output.contains("10.0% overwritten"));
        assert!(matches!(
            mouse.at(terminal.backend().buffer().area, 1, 1),
            Some((ListTarget::HotspotHeader(HotspotSort::Severity), 0))
        ));
    }
}
