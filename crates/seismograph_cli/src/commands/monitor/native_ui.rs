// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native allocator navigation and structural presentation.

#[cfg(test)]
mod acceptance;

use crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use seismograph_rallocator::native::{Freshness, Observation, ObservationSource, Owner, Ranges, Snapshot};

use super::help::Context;
use super::mouse::{ListTarget, MouseRows};
use super::ui::wrapped_detail_with_limit;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Depth {
    #[default]
    Root,
    Memory,
    MemoryDetail,
    Subsystems,
    Classes,
    Class,
    Detail,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Navigation {
    pub(super) root: usize,
    pub(super) selected_owner: Option<u64>,
    pub(super) subsystem: usize,
    pub(super) class: usize,
    pub(super) memory: usize,
    pub(super) scroll: usize,
    pub(super) depth: Depth,
}

const SUBSYSTEMS: [&str; 6] = [
    "Small classes",
    "Large ranges",
    "Local ranges",
    "Metadata cache",
    "Remote returns",
    "Observation",
];
const MEMORY: [&str; 4] = ["Reserved", "Committed", "Resident", "Swapped"];

impl Navigation {
    pub(super) fn reconcile(&mut self, snapshot: Option<&Snapshot>) {
        if let Some(id) = self.selected_owner {
            if let Some(index) = snapshot.and_then(|snapshot| snapshot.owners.iter().position(|owner| owner.id == id)) {
                self.root = index + 2;
            } else {
                self.root = 0;
                self.depth = Depth::Root;
                self.selected_owner = None;
                self.scroll = 0;
            }
        } else {
            self.root = self.root.min(snapshot.map_or(1, |snapshot| snapshot.owners.len() + 1));
            self.remember_owner(snapshot);
        }
    }

    fn remember_owner(&mut self, snapshot: Option<&Snapshot>) {
        self.selected_owner = self
            .root
            .checked_sub(2)
            .and_then(|index| snapshot.and_then(|snapshot| snapshot.owners.get(index)))
            .map(|owner| owner.id);
    }

    pub(super) fn key(&mut self, key: KeyCode, snapshot: Option<&Snapshot>) -> bool {
        if snapshot.is_some() {
            self.reconcile(snapshot);
        }
        let count = match self.depth {
            Depth::Root => snapshot.map_or(2, |snapshot| snapshot.owners.len() + 2),
            Depth::Memory => MEMORY.len(),
            Depth::Subsystems => SUBSYSTEMS.len(),
            Depth::Classes => seismograph_rallocator::native::CLASS_COUNT,
            Depth::Class | Depth::Detail | Depth::MemoryDetail => 0,
        };
        if matches!(key, KeyCode::Esc | KeyCode::Backspace) {
            return self.back();
        }
        if key == KeyCode::Enter {
            self.depth = match self.depth {
                Depth::Root if self.root == 0 => Depth::Memory,
                Depth::Root if self.root == 1 => Depth::Detail,
                Depth::Root => Depth::Subsystems,
                Depth::Memory => Depth::MemoryDetail,
                Depth::Subsystems if self.subsystem == 0 => Depth::Classes,
                Depth::Subsystems => Depth::Detail,
                Depth::Classes => Depth::Class,
                depth => depth,
            };
            self.scroll = 0;
            return true;
        }
        if count == 0 && matches!(key, KeyCode::PageUp | KeyCode::PageDown) {
            self.scroll = if key == KeyCode::PageUp {
                self.scroll.saturating_sub(5)
            } else {
                self.scroll.saturating_add(5)
            };
            return true;
        }
        if count == 0 {
            self.scroll = match key {
                KeyCode::Up => self.scroll.saturating_sub(1),
                KeyCode::Down => self.scroll.saturating_add(1),
                KeyCode::Home => 0,
                KeyCode::End => usize::MAX,
                _ => return false,
            };
        } else {
            let selected = match self.depth {
                Depth::Root => &mut self.root,
                Depth::Memory => &mut self.memory,
                Depth::Subsystems => &mut self.subsystem,
                _ => &mut self.class,
            };
            *selected = match key {
                KeyCode::Up => selected.saturating_sub(1),
                KeyCode::Down => selected.saturating_add(1).min(count - 1),
                KeyCode::Home => 0,
                KeyCode::End => count - 1,
                KeyCode::PageUp => selected.saturating_sub(5),
                KeyCode::PageDown => selected.saturating_add(5).min(count - 1),
                _ => return false,
            };
            self.scroll = 0;
            if self.depth == Depth::Root {
                self.subsystem = 0;
                self.class = 0;
                self.remember_owner(snapshot);
            }
        }
        true
    }

    fn back(&mut self) -> bool {
        self.depth = match self.depth {
            Depth::Root => return false,
            Depth::MemoryDetail => Depth::Memory,
            Depth::Class => Depth::Classes,
            Depth::Classes => Depth::Subsystems,
            Depth::Detail if self.root >= 2 => Depth::Subsystems,
            _ => Depth::Root,
        };
        self.scroll = 0;
        true
    }

    pub(super) fn click(&mut self, target: ListTarget, index: usize) {
        self.scroll = 0;
        match target {
            ListTarget::HeapBuckets => {
                self.root = index;
                self.selected_owner = None;
                self.depth = Depth::Root;
                self.subsystem = 0;
                self.class = 0;
            }
            ListTarget::NativeSubsystems => {
                self.subsystem = index;
                self.depth = Depth::Subsystems;
            }
            ListTarget::NativeClasses => {
                self.class = index;
                self.depth = Depth::Classes;
            }
            ListTarget::NativeMemory => {
                self.memory = index;
                self.depth = Depth::Memory;
            }
            _ => {}
        }
    }

    pub(super) fn focus_list(&mut self, target: ListTarget) {
        self.depth = match target {
            ListTarget::HeapBuckets => Depth::Root,
            ListTarget::NativeSubsystems => Depth::Subsystems,
            ListTarget::NativeClasses => Depth::Classes,
            ListTarget::NativeMemory => Depth::Memory,
            _ => self.depth,
        };
    }

    pub(super) const fn help(self) -> Context {
        if self.root == 0 {
            return Context::NativeMemory;
        }
        if self.root == 1 {
            return Context::NativeGlobal;
        }
        match self.depth {
            Depth::Root => Context::NativeOwner,
            Depth::Classes | Depth::Class => Context::NativeClasses,
            _ => match self.subsystem {
                0 => Context::NativeClasses,
                1 => Context::NativeRanges,
                2 | 3 => Context::NativeCaches,
                4 => Context::NativeReturns,
                _ => Context::NativeObservation,
            },
        }
    }
}

pub(super) fn bytes(value: u128) -> String {
    let units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut unit = 0;
    let mut scale = 1_u128;
    while unit + 1 < units.len() && value / scale >= 1024 {
        scale *= 1024;
        unit += 1;
    }
    if unit == 0 {
        format!("{value} B")
    } else {
        let fraction = ((value % scale) * 100 + scale / 2) / scale;
        format!("{}.{:02} {}", value / scale + fraction / 100, fraction % 100, units[unit])
    }
}

pub(super) fn badge(snapshot: &Snapshot, owner: &Owner) -> &'static str {
    match owner.source {
        ObservationSource::Busy => match owner.freshness(snapshot) {
            Freshness::Unknown | Freshness::Current | Freshness::IdleInspection => "Busy",
            _ => "Busy / Older",
        },
        ObservationSource::Unavailable => "Unavailable",
        _ => match owner.freshness(snapshot) {
            Freshness::Unknown => "Unknown",
            Freshness::Current | Freshness::IdleInspection => "Observed",
            _ => "Older",
        },
    }
}

fn owner_label(snapshot: &Snapshot, index: usize) -> String {
    let owner = &snapshot.owners[index];
    format!(
        "Owner {} {} [{}]",
        index + 1,
        if owner.leased { "Active" } else { "Idle" },
        badge(snapshot, owner)
    )
}

fn frame(title: String, focused: bool) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused { Color::Cyan } else { Color::DarkGray }))
}

fn list(
    terminal: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    labels: &[String],
    selected: usize,
    target: ListTarget,
    block: Block<'static>,
) {
    let selected = selected.min(labels.len().saturating_sub(1));
    let visible = usize::from(block.inner(area).height).max(1);
    let first = selected.saturating_sub(visible - 1);
    let rows = labels.iter().skip(first).take(visible).map(|label| ListItem::new(label.clone()));
    let mut state = ListState::default().with_selected(Some(selected - first));
    terminal.render_stateful_widget(
        List::new(rows)
            .block(block)
            .highlight_symbol("› ")
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan)),
        area,
        &mut state,
    );
    mouse.register(area, 0, first + state.offset(), labels.len(), target);
}

pub(super) fn draw(
    terminal: &mut ratatui::Frame<'_>,
    mouse: &MouseRows,
    area: Rect,
    snapshot: Option<&Snapshot>,
    unavailable: Option<&str>,
    mut nav: Navigation,
) {
    nav.reconcile(snapshot);
    let [top, body] = Layout::vertical([Constraint::Length(11), Constraint::Min(5)]).areas(area);
    draw_flow(terminal, top, snapshot, unavailable.is_some());
    let [left, right] =
        Layout::horizontal([Constraint::Length(area.width.saturating_sub(40).clamp(18, 34)), Constraint::Min(15)]).areas(body);
    let mut labels = vec!["Memory".into(), "Global backend".into()];
    if let Some(snapshot) = snapshot {
        labels.extend((0..snapshot.owners.len()).map(|index| owner_label(snapshot, index)));
    }
    list(
        terminal,
        mouse,
        left,
        &labels,
        nav.root,
        ListTarget::HeapBuckets,
        frame(" Allocator · Enter › ".into(), nav.depth == Depth::Root),
    );
    if let Some(error) = unavailable {
        let lines = error.lines().map(str::to_owned).collect::<Vec<_>>();
        detail(
            terminal,
            mouse,
            right,
            " Source / capture error · Enter details ".into(),
            &lines,
            nav.scroll,
            nav.depth != Depth::Root,
        );
        return;
    }
    let Some(snapshot) = snapshot else {
        draw_unknown(terminal, mouse, right, nav);
        return;
    };
    if nav.root == 0 {
        draw_memory(terminal, mouse, right, snapshot, nav);
    } else if nav.root == 1 {
        detail(
            terminal,
            mouse,
            right,
            " Global backend ".into(),
            &global_lines(snapshot),
            nav.scroll,
            nav.depth != Depth::Root,
        );
    } else if let Some(owner) = snapshot.owners.get(nav.root - 2) {
        draw_owner(terminal, mouse, right, snapshot, owner, nav);
    }
}

fn draw_unknown(terminal: &mut ratatui::Frame<'_>, mouse: &MouseRows, area: Rect, nav: Navigation) {
    if nav.root != 0 {
        detail(
            terminal,
            mouse,
            area,
            " Global backend ".into(),
            &["Unknown".into()],
            0,
            nav.depth != Depth::Root,
        );
        return;
    }
    let [choices, data] = Layout::horizontal([Constraint::Length(17), Constraint::Min(10)]).areas(area);
    list(
        terminal,
        mouse,
        choices,
        &MEMORY.map(str::to_owned),
        nav.memory,
        ListTarget::NativeMemory,
        frame(" Memory ".into(), nav.depth == Depth::Memory),
    );
    detail(
        terminal,
        mouse,
        data,
        format!(" {} · F1 ", MEMORY[nav.memory.min(3)]),
        &["— [Unknown]".into()],
        0,
        nav.depth == Depth::MemoryDetail,
    );
}

fn observation_counts(snapshot: &Snapshot) -> String {
    let mut counts = [0_usize; 5];
    for owner in &snapshot.owners {
        let index = match owner.source {
            ObservationSource::Busy => 3,
            ObservationSource::Unavailable => 4,
            _ => match owner.freshness(snapshot) {
                Freshness::Current | Freshness::IdleInspection => 0,
                Freshness::Unknown => 2,
                _ => 1,
            },
        };
        counts[index] += 1;
    }
    format!(
        "Observed {}  Older {}  Unknown {}  Busy {}  Unavailable {}",
        counts[0], counts[1], counts[2], counts[3], counts[4]
    )
}

fn draw_flow(terminal: &mut ratatui::Frame<'_>, area: Rect, snapshot: Option<&Snapshot>, error: bool) {
    let lines = snapshot.map_or_else(
        || vec!["OS reserved  — [Unknown]".into(), "OS → Global backend → Owner frontends".into()],
        |snapshot| {
            let total = u128::from(snapshot.global.reserved_bytes) + u128::from(snapshot.global.pagemap_reserved_bytes);
            let mut lines = vec![
                format!("OS reserved       {}", bytes(total)),
                format!(
                    "Allocator ranges  {}    Page-map VA  {}",
                    bytes(snapshot.global.reserved_bytes.into()),
                    bytes(snapshot.global.pagemap_reserved_bytes.into())
                ),
                format!(
                    "Owners {} / {} [{}]   Active {}   Idle {}",
                    snapshot.owners.len(),
                    snapshot.owner_count,
                    coverage(snapshot.owners_complete),
                    snapshot.owners.iter().filter(|owner| owner.leased).count(),
                    snapshot.owners.iter().filter(|owner| !owner.leased).count(),
                ),
                observation_counts(snapshot),
            ];
            if area.width >= 100 {
                lines.extend([
                    "┌───────────┐       ┌────────────────┐       ┌─────────────────┐".into(),
                    "│    OS     │ ────› │ Global backend │ ────› │ Owner frontends │".into(),
                    "└───────────┘       └────────────────┘       └────────┬────────┘".into(),
                    "     ‹── Cached ranges              Small slabs · Large ranges · Local caches ⇄ Returns".into(),
                ]);
            } else {
                lines.extend([
                    "OS → Global backend → Owners".into(),
                    "       ↳ Slabs · Ranges · Caches ⇄ Returns".into(),
                ]);
            }
            lines
        },
    );
    terminal.render_widget(
        Paragraph::new(lines.join("\n"))
            .block(
                frame(
                    if error {
                        " Native allocator [Error] ".into()
                    } else {
                        " Native allocator ".into()
                    },
                    false,
                )
                .border_style(Style::default().fg(if error { Color::Red } else { Color::DarkGray })),
            )
            .wrap(ratatui::widgets::Wrap { trim: false }),
        area,
    );
}

fn detail(terminal: &mut ratatui::Frame<'_>, mouse: &MouseRows, area: Rect, title: String, lines: &[String], scroll: usize, focused: bool) {
    let (paragraph, limit) = wrapped_detail_with_limit(lines, frame(title, focused), area, scroll);
    mouse.native_scroll_limit.set(limit);
    terminal.render_widget(paragraph, area);
}

fn global_lines(snapshot: &Snapshot) -> Vec<String> {
    let global = snapshot.global;
    let mut lines = vec![
        format!("Allocator ranges   {}", bytes(global.reserved_bytes.into())),
        format!("Page-map VA        {}", bytes(global.pagemap_reserved_bytes.into())),
        format!(
            "Cached capacity    {} [{}]",
            bytes(global.ranges.observed_bytes()),
            coverage(global.ranges.complete)
        ),
        format!("Local boundary     {}", bytes(global.local_limit_bytes.into())),
        format!("Refill ceiling     {}", bytes(global.global_refill_bytes.into())),
        String::new(),
    ];
    lines.extend(range_bins(&global.ranges));
    lines
}

fn coverage(complete: bool) -> &'static str {
    if complete { "Complete" } else { "Partial" }
}

fn draw_memory(terminal: &mut ratatui::Frame<'_>, mouse: &MouseRows, area: Rect, snapshot: &Snapshot, nav: Navigation) {
    let [choices, data] = Layout::horizontal([Constraint::Length(17), Constraint::Min(10)]).areas(area);
    list(
        terminal,
        mouse,
        choices,
        &MEMORY.map(str::to_owned),
        nav.memory,
        ListTarget::NativeMemory,
        frame(" Memory ".into(), nav.depth == Depth::Memory),
    );
    let lines = if nav.memory == 0 {
        let total = u128::from(snapshot.global.reserved_bytes) + u128::from(snapshot.global.pagemap_reserved_bytes);
        vec![
            format!("OS reserved        {}", bytes(total)),
            format!("Allocator ranges   {}", bytes(snapshot.global.reserved_bytes.into())),
            format!("Page-map VA        {}", bytes(snapshot.global.pagemap_reserved_bytes.into())),
            String::new(),
            reservation_bar(snapshot),
            "▓ Allocator ranges   ░ Page-map VA".into(),
            String::new(),
            format!(
                "Global cached      {} [{}]",
                bytes(snapshot.global.ranges.observed_bytes()),
                coverage(snapshot.global.ranges.complete)
            ),
        ]
    } else {
        vec![format!("{}             — [Unknown]", MEMORY[nav.memory.min(3)])]
    };
    detail(
        terminal,
        mouse,
        data,
        format!(" {} · F1 ", MEMORY[nav.memory.min(3)]),
        &lines,
        nav.scroll,
        nav.depth == Depth::MemoryDetail,
    );
}

fn reservation_bar(snapshot: &Snapshot) -> String {
    let allocator = u128::from(snapshot.global.reserved_bytes);
    let total = allocator + u128::from(snapshot.global.pagemap_reserved_bytes);
    if total == 0 {
        return "OS reserved  0 B".into();
    }
    let cells = usize::try_from(allocator * 24 / total).unwrap_or(24);
    format!("{}{}", "▓".repeat(cells), "░".repeat(24 - cells))
}

fn owner_overview(snapshot: &Snapshot, owner: &Owner) -> Vec<String> {
    let Some(observation) = &owner.observation else {
        return vec![format!("[{}]", badge(snapshot, owner))];
    };
    let slabs = observation
        .classes
        .iter()
        .map(|class| u128::from(class.observed_slabs))
        .sum::<u128>();
    let capacity = observation
        .classes
        .iter()
        .try_fold(0_u128, |total, class| {
            total.checked_add(u128::from(class.slab_bytes) * u128::from(class.observed_slabs))
        })
        .map_or_else(|| "— [Unknown]".into(), bytes);
    vec![
        "Global backend → Local ranges → Small slabs / Large ranges".into(),
        "                      Metadata cache       ⇄ Remote returns".into(),
        String::new(),
        format!("Small slabs        {capacity} · {slabs} [{}]", coverage(observation.slabs_complete)),
        format!(
            "Large ranges       {} [{}]",
            bytes(observation.large.observed_bytes()),
            coverage(observation.large.complete)
        ),
        format!(
            "Local ranges       {} [{}]",
            bytes(observation.local.ranges.observed_bytes()),
            coverage(observation.local.ranges.complete)
        ),
        format!(
            "Metadata cache     {} [{}]",
            bytes(observation.local.metadata.observed_bytes()),
            coverage(observation.local.metadata.complete)
        ),
        format!("Open returns       {} objects", observation.remote.open_objects),
    ]
}

fn draw_owner(terminal: &mut ratatui::Frame<'_>, mouse: &MouseRows, area: Rect, snapshot: &Snapshot, owner: &Owner, nav: Navigation) {
    let title = format!(" Owner {} [{}] ", nav.root - 1, badge(snapshot, owner));
    if nav.depth == Depth::Root {
        detail(terminal, mouse, area, title, &owner_overview(snapshot, owner), 0, false);
        return;
    }
    if nav.depth == Depth::Subsystems {
        let [choices, data] = Layout::horizontal([Constraint::Length(22), Constraint::Min(10)]).areas(area);
        list(
            terminal,
            mouse,
            choices,
            &SUBSYSTEMS.map(str::to_owned),
            nav.subsystem,
            ListTarget::NativeSubsystems,
            frame(" Subsystems · Enter › ".into(), true),
        );
        detail(
            terminal,
            mouse,
            data,
            title,
            &subsystem_lines(owner, nav.subsystem),
            nav.scroll,
            false,
        );
    } else if matches!(nav.depth, Depth::Classes | Depth::Class) {
        draw_classes(terminal, mouse, area, snapshot, owner, nav);
    } else {
        detail(
            terminal,
            mouse,
            area,
            format!("{} / {} · Esc ‹ ", title.trim(), SUBSYSTEMS[nav.subsystem.min(5)]),
            &subsystem_lines(owner, nav.subsystem),
            nav.scroll,
            true,
        );
    }
}

fn draw_classes(terminal: &mut ratatui::Frame<'_>, mouse: &MouseRows, area: Rect, snapshot: &Snapshot, owner: &Owner, nav: Navigation) {
    let Some(observation) = &owner.observation else {
        detail(terminal, mouse, area, " Small classes ".into(), &["Unknown".into()], 0, true);
        return;
    };
    let expanded = area.width >= 64;
    let [choices, data] = if area.width < 40 {
        Layout::vertical([Constraint::Length(5), Constraint::Min(3)]).areas(area)
    } else {
        Layout::horizontal([Constraint::Length(if expanded { 24 } else { 18 }), Constraint::Min(10)]).areas(area)
    };
    let labels = observation
        .classes
        .iter()
        .enumerate()
        .map(|(index, class)| {
            if expanded {
                format!("C{index:02} {} · {}", bytes(class.object_bytes.into()), class.observed_slabs)
            } else {
                format!("C{index:02} {}", bytes(class.object_bytes.into()))
            }
        })
        .collect::<Vec<_>>();
    list(
        terminal,
        mouse,
        choices,
        &labels,
        nav.class,
        ListTarget::NativeClasses,
        frame(
            format!(
                " {} [{}] ",
                if expanded { "Classes" } else { "C" },
                coverage(observation.slabs_complete)
            ),
            nav.depth == Depth::Classes,
        ),
    );
    let class = observation.classes[nav.class.min(observation.classes.len() - 1)];
    let lines = if class.object_bytes == 0 {
        vec!["Unknown".into()]
    } else {
        let mut lines = vec![
            format!("Object size        {}", bytes(class.object_bytes.into())),
            format!("Slab size          {}", bytes(class.slab_bytes.into())),
            format!("Slots / slab       {}", class.capacity),
            format!("Slabs              {}", class.observed_slabs),
            format!("Available slabs    {}", class.available_slabs),
            format!("Empty slabs        {}", class.empty_slabs),
            format!("Fast list          {}", if class.fast_nonempty { "Ready" } else { "Empty" }),
            format!("Geometry           {} × {}", class.capacity, bytes(class.object_bytes.into())),
        ];
        lines.extend(slab_geometry(class.slab_bytes, class.capacity));
        lines
    };
    detail(
        terminal,
        mouse,
        data,
        format!(" C{} [{}] ", nav.class, badge(snapshot, owner)),
        &lines,
        nav.scroll,
        nav.depth == Depth::Class,
    );
}

fn slab_geometry(slab_bytes: u64, capacity: u64) -> [String; 3] {
    let label = format!(" Slab {} ", bytes(slab_bytes.into()));
    let slots = format!(" [slot] × {capacity} ");
    let width = label.chars().count().max(slots.chars().count());
    [
        format!("┌{}{}┐", label, "─".repeat(width - label.chars().count())),
        format!("│{slots:width$}│"),
        format!("└{}┘", "─".repeat(width)),
    ]
}

fn range_bins(ranges: &Ranges) -> Vec<String> {
    let maximum = ranges.counts.iter().copied().max().unwrap_or(0);
    ranges
        .counts
        .iter()
        .enumerate()
        .filter(|(_, count)| **count != 0)
        .map(|(exponent, count)| {
            let filled = if maximum == 0 {
                0
            } else {
                (u128::from(*count) * 12 / u128::from(maximum)) as usize
            };
            format!("{:>10}  {:>5}  {}", bytes(1_u128 << exponent), count, "█".repeat(filled))
        })
        .collect()
}

fn ranges(title: &str, ranges: &Ranges) -> Vec<String> {
    let mut lines = vec![format!(
        "{title}  {} [{}]",
        bytes(ranges.observed_bytes()),
        coverage(ranges.complete)
    )];
    lines.extend(range_bins(ranges));
    lines
}

fn subsystem_lines(owner: &Owner, selected: usize) -> Vec<String> {
    if selected == 5 {
        let mut lines = vec![
            format!("Endpoint           0x{:x}", owner.id),
            format!("Lease              {}", if owner.leased { "Active" } else { "Idle" }),
        ];
        if let Some(observation) = &owner.observation {
            lines.push(format!(
                "Last contributor   {}",
                if observation.thread_id == 0 {
                    "—".into()
                } else {
                    observation.thread_id.to_string()
                }
            ));
        }
        return lines;
    }
    let Some(observation) = &owner.observation else {
        return vec!["Unknown".into()];
    };
    match selected {
        0 => vec![
            format!("Classes            44 [{}]", coverage(observation.slabs_complete)),
            format!(
                "Slabs              {}",
                observation
                    .classes
                    .iter()
                    .map(|class| u128::from(class.observed_slabs))
                    .sum::<u128>()
            ),
        ],
        1 => ranges("Large ranges", &observation.large),
        2 => ranges("Local ranges", &observation.local.ranges),
        3 => ranges("Metadata cache", &observation.local.metadata),
        _ => remote_lines(observation),
    }
}

fn remote_lines(observation: &Observation) -> Vec<String> {
    let remote = observation.remote;
    vec![
        "Frontend ──› Open rings ──› Messages ──› Owner inbox".into(),
        format!("Open rings         {}", remote.open_rings),
        format!("Open objects       {}", remote.open_objects),
        format!("Message buckets    {}", remote.outgoing_lists),
        format!("Messages           {} [{}]", remote.messages, coverage(remote.complete)),
        format!("Message objects    {}", remote.message_objects),
        format!("Message capacity   {}", bytes(remote.message_bytes.into())),
        format!("Batch budget       {}", remote.budget_remaining),
        format!(
            "Inbox work         {}",
            if remote.incoming_front == remote.incoming_back {
                "Unknown"
            } else {
                "Potential"
            }
        ),
    ]
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    pub(super) fn render(snapshot: Option<&Snapshot>, nav: Navigation, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw(frame, &MouseRows::default(), frame.area(), snapshot, None, nav))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(usize::from(width))
            .map(|row| row.iter().map(ratatui::buffer::Cell::symbol).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn navigation_scrolls_saturates_and_rejects_unrelated_keys_and_targets() {
        let snapshot = crate::native_view::fixture::snapshot();
        let mut nav = Navigation::default();
        assert!(!nav.key(KeyCode::Char('x'), None));
        assert!(nav.key(KeyCode::PageDown, None));
        assert_eq!(nav.root, 1);
        assert!(nav.key(KeyCode::PageUp, None));
        assert_eq!(nav.root, 0);
        nav.key(KeyCode::Enter, Some(&snapshot));
        nav.key(KeyCode::Enter, Some(&snapshot));
        assert_eq!(nav.depth, Depth::MemoryDetail);
        nav.key(KeyCode::PageDown, Some(&snapshot));
        assert_eq!(nav.scroll, 5);
        nav.key(KeyCode::PageUp, Some(&snapshot));
        assert_eq!(nav.scroll, 0);
        assert!(nav.key(KeyCode::Up, Some(&snapshot)));
        assert!(nav.key(KeyCode::Down, Some(&snapshot)));
        assert_eq!(nav.scroll, 1);
        assert!(nav.key(KeyCode::Home, Some(&snapshot)));
        assert_eq!(nav.scroll, 0);
        assert!(!nav.key(KeyCode::Char('x'), Some(&snapshot)));
        assert!(nav.key(KeyCode::Esc, Some(&snapshot)));
        assert_eq!(nav.depth, Depth::Memory);
        nav.click(ListTarget::NativeMemory, 3);
        assert_eq!(nav.memory, 3);
        nav.click(ListTarget::Threads, 99);
        assert_eq!(nav.memory, 3);
        nav.focus_list(ListTarget::NativeMemory);
        assert_eq!(nav.depth, Depth::Memory);
        nav.focus_list(ListTarget::Threads);
        assert_eq!(nav.depth, Depth::Memory);
        nav.focus_list(ListTarget::HeapBuckets);
        assert_eq!(nav.depth, Depth::Root);
        nav.focus_list(ListTarget::NativeSubsystems);
        assert_eq!(nav.depth, Depth::Subsystems);
        nav.click(ListTarget::HeapBuckets, 2);
        nav.click(ListTarget::NativeSubsystems, 1);
        assert_eq!((nav.subsystem, nav.depth), (1, Depth::Subsystems));
        nav.key(KeyCode::Enter, Some(&snapshot));
        assert!(nav.key(KeyCode::Enter, Some(&snapshot)));
        assert_eq!(nav.depth, Depth::Detail);
        nav.click(ListTarget::NativeClasses, 43);
        assert_eq!((nav.class, nav.depth), (43, Depth::Classes));
        assert_eq!(nav.help(), Context::NativeClasses);
        let mut observation = snapshot.owners[0].observation.unwrap();
        observation.remote.incoming_back = observation.remote.incoming_front;
        assert!(remote_lines(&observation).iter().any(|line| line == "Inbox work         Unknown"));
        nav.root = 2;
        nav.subsystem = 5;
        nav.depth = Depth::Detail;
        assert_eq!(nav.help(), Context::NativeObservation);
        let mut empty = snapshot.clone();
        empty.global.reserved_bytes = 0;
        empty.global.pagemap_reserved_bytes = 0;
        assert_eq!(reservation_bar(&empty), "OS reserved  0 B");
    }

    #[test]
    fn unknown_owner_and_uninitialized_class_details_stay_unknown_at_narrow_widths() {
        let snapshot = crate::native_view::fixture::snapshot();
        for (root, class) in [(3, 1), (2, 0)] {
            for width in [36, 60, 120] {
                let nav = Navigation {
                    root,
                    class,
                    depth: Depth::Class,
                    ..Default::default()
                };
                let screen = render(Some(&snapshot), nav, width, 24);
                assert!(screen.contains("Unknown"), "{screen}");
                assert!(!screen.contains("Object size"), "{screen}");
                assert_eq!(screen.lines().count(), 24);
                assert!(screen.lines().all(|line| line.chars().count() == usize::from(width)));
            }
        }
    }

    #[test]
    fn write_native_visual_acceptance_screens() -> std::io::Result<()> {
        let mut snapshot = crate::native_view::fixture::snapshot();
        snapshot.global.reserved_bytes = 64 * 1024 * 1024;
        snapshot.global.pagemap_reserved_bytes = 256 * 1024 * 1024 * 1024;
        snapshot.owners[0]
            .observation
            .as_mut()
            .unwrap()
            .classes
            .fill(seismograph_rallocator::native::ClassState {
                object_bytes: 16,
                slab_bytes: 16384,
                capacity: 1024,
                observed_slabs: 1,
                ..Default::default()
            });
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join("native-ui");
        std::fs::create_dir_all(&directory)?;
        for (name, nav, width, height) in [
            ("memory", Navigation::default(), 140, 38),
            (
                "global",
                Navigation {
                    root: 1,
                    depth: Depth::Detail,
                    ..Default::default()
                },
                140,
                38,
            ),
            (
                "owner",
                Navigation {
                    root: 2,
                    ..Default::default()
                },
                140,
                38,
            ),
            (
                "classes",
                Navigation {
                    root: 2,
                    class: 43,
                    depth: Depth::Classes,
                    ..Default::default()
                },
                140,
                38,
            ),
            (
                "class-narrow",
                Navigation {
                    root: 2,
                    class: 43,
                    depth: Depth::Class,
                    scroll: usize::MAX,
                    ..Default::default()
                },
                80,
                24,
            ),
            (
                "returns",
                Navigation {
                    root: 2,
                    subsystem: 4,
                    depth: Depth::Detail,
                    ..Default::default()
                },
                140,
                38,
            ),
            (
                "unknown",
                Navigation {
                    root: 3,
                    ..Default::default()
                },
                80,
                24,
            ),
        ] {
            let screen = render(Some(&snapshot), nav, width, height);
            assert_eq!(screen.lines().count(), usize::from(height), "{name}");
            assert!(screen.lines().all(|line| line.chars().count() == usize::from(width)), "{name}");
            assert!(!screen.contains("NaN"), "{name}");
            std::fs::write(directory.join(format!("{name}.txt")), &screen)?;
            assert_eq!(std::fs::read_to_string(directory.join(format!("{name}.txt")))?, screen);
        }
        Ok(())
    }

    #[test]
    fn subsystem_details_preserve_unknown_and_complete_range_evidence() {
        let mut snapshot = crate::native_view::fixture::snapshot();
        let owner = &mut snapshot.owners[0];
        for (selected, title) in [(1, "Large ranges"), (2, "Local ranges"), (3, "Metadata cache")] {
            assert!(subsystem_lines(owner, selected)[0].contains(title));
        }
        let endpoint = subsystem_lines(owner, 5);
        assert!(endpoint[0].contains(&format!("0x{:x}", owner.id)));
        assert!(endpoint[1].contains("Active"));
        owner.observation.as_mut().unwrap().thread_id = 0;
        assert!(subsystem_lines(owner, 5)[2].contains('—'));
        let mut range = Ranges {
            complete: true,
            ..Default::default()
        };
        range.counts[14] = 2;
        let detail = ranges("Ranges", &range);
        assert!(detail[0].contains("[Complete]"));
        assert!(detail[1].contains("16.00 KiB"));
        assert_eq!(detail[1].matches('█').count(), 12);
        owner.observation = None;
        assert_eq!(subsystem_lines(owner, 0), ["Unknown"]);
        assert_eq!(subsystem_lines(owner, 5).len(), 2);
    }

    #[test]
    fn every_owner_subsystem_and_class_is_reachable_and_backtracks() {
        let snapshot = crate::native_view::fixture::snapshot();
        for (subsystem, context) in [
            Context::NativeClasses,
            Context::NativeRanges,
            Context::NativeCaches,
            Context::NativeCaches,
            Context::NativeReturns,
            Context::NativeObservation,
        ]
        .into_iter()
        .enumerate()
        {
            let mut nav = Navigation {
                root: 2,
                ..Default::default()
            };
            nav.key(KeyCode::Enter, Some(&snapshot));
            for _ in 0..subsystem {
                nav.key(KeyCode::Down, Some(&snapshot));
            }
            assert_eq!(nav.help(), context);
            nav.key(KeyCode::Enter, Some(&snapshot));
            if subsystem == 0 {
                nav.key(KeyCode::End, Some(&snapshot));
                assert_eq!(nav.class, 43);
                nav.key(KeyCode::Enter, Some(&snapshot));
                assert_eq!(nav.depth, Depth::Class);
                nav.key(KeyCode::Esc, Some(&snapshot));
                assert_eq!(nav.depth, Depth::Classes);
            } else {
                assert_eq!(nav.depth, Depth::Detail);
            }
            nav.key(KeyCode::Backspace, Some(&snapshot));
            assert_eq!(nav.depth, Depth::Subsystems);
            nav.key(KeyCode::Esc, Some(&snapshot));
            assert_eq!(nav.depth, Depth::Root);
            assert!(!nav.key(KeyCode::Esc, Some(&snapshot)));
        }
    }

    #[test]
    fn memory_categories_do_not_invent_physical_measurements() {
        let snapshot = crate::native_view::fixture::snapshot();
        let mut nav = Navigation::default();
        nav.key(KeyCode::Enter, Some(&snapshot));
        assert!(render(Some(&snapshot), nav, 120, 30).contains("Page-map VA"));
        for category in MEMORY.iter().skip(1) {
            nav.key(KeyCode::Down, Some(&snapshot));
            let output = render(Some(&snapshot), nav, 120, 30);
            assert!(output.contains(category));
            assert!(output.contains("— [Unknown]"));
        }
        assert_eq!(nav.memory, 3);
        nav.key(KeyCode::Esc, Some(&snapshot));
        nav.key(KeyCode::Down, Some(&snapshot));
        nav.key(KeyCode::Enter, Some(&snapshot));
        assert_eq!(nav.depth, Depth::Detail);
    }

    #[test]
    fn normal_panels_have_no_age_or_caveat_prose_and_help_has_meanings() {
        let snapshot = crate::native_view::fixture::snapshot();
        for root in 0..snapshot.owners.len() + 2 {
            let output = render(
                Some(&snapshot),
                Navigation {
                    root,
                    ..Default::default()
                },
                140,
                40,
            );
            for absent in ["Age", "session", "generation", "limitations", "not app", "not physical", "not zero"] {
                assert!(!output.contains(absent), "unexpected {absent}: {output}");
            }
        }
        let help = super::super::help_content::document(Context::NativeMemory, true)
            .iter()
            .flat_map(|section| section.entries)
            .map(|entry| entry.explanation)
            .collect::<String>();
        for meaning in ["128-bit", "256 GiB", "physical", "metadata", "not zero", "overlap"] {
            assert!(help.contains(meaning), "missing {meaning}");
        }
    }

    #[test]
    fn coverage_and_missing_source_remain_explicit() {
        let mut snapshot = crate::native_view::fixture::snapshot();
        let output = render(Some(&snapshot), Navigation::default(), 160, 35);
        for badge in ["Observed", "Older", "Unknown", "Busy", "Busy / Older", "Partial"] {
            assert!(output.contains(badge), "missing {badge}");
        }
        snapshot.owners[0].source = ObservationSource::Unavailable;
        snapshot.owners[0].observation = None;
        let output = render(
            Some(&snapshot),
            Navigation {
                root: 2,
                ..Default::default()
            },
            100,
            24,
        );
        assert!(output.contains("Unavailable"));
        assert!(!output.contains("Slabs              0"));
        assert!(render(None, Navigation::default(), 80, 24).contains("Unknown"));
    }

    #[test]
    fn mouse_selection_and_pages_preserve_full_class_indices_on_narrow_screens() {
        let mut snapshot = crate::native_view::fixture::snapshot();
        snapshot.owners[0]
            .observation
            .as_mut()
            .unwrap()
            .classes
            .fill(seismograph_rallocator::native::ClassState {
                object_bytes: 16,
                slab_bytes: 16384,
                capacity: 1024,
                observed_slabs: 1,
                ..Default::default()
            });
        let mut nav = Navigation {
            root: 2,
            ..Default::default()
        };
        nav.click(ListTarget::NativeSubsystems, 0);
        nav.key(KeyCode::Enter, Some(&snapshot));
        nav.key(KeyCode::End, Some(&snapshot));
        assert!(render(Some(&snapshot), nav, 80, 24).contains("C43"));
        nav.click(ListTarget::NativeClasses, 42);
        nav.key(KeyCode::Enter, Some(&snapshot));
        nav.key(KeyCode::End, Some(&snapshot));
        assert!(render(Some(&snapshot), nav, 80, 24).contains("[slot]"));
        nav.key(KeyCode::Esc, Some(&snapshot));
        nav.key(KeyCode::PageUp, Some(&snapshot));
        assert_eq!(nav.class, 37);
        nav.click(ListTarget::NativeMemory, 3);
        assert_eq!((nav.depth, nav.memory), (Depth::Memory, 3));
        nav.click(ListTarget::HeapBuckets, 1);
        assert_eq!((nav.depth, nav.root), (Depth::Root, 1));
    }
}
