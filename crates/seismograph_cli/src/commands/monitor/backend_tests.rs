// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consumer-boundary tests for backend state, filtering, and monitor navigation.

use crossterm::event::KeyCode;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use seismograph::snapshot::SourceSnapshot;

use super::app::{AllocatorBackend, App, MonitorTab, Screen};
use super::data::CapturedSnapshot;
use super::filter::{FilterSpec, RuntimeStackMode};
use super::mimalloc_ui::Depth;
use super::mouse::ListTarget;
use crate::mimalloc_view::fixture;

fn capture(sources: &[SourceSnapshot]) -> Box<CapturedSnapshot> {
    let bytes = fixture::container(sources);
    super::snapshot::prepare(seismograph::snapshot::decode(&bytes).unwrap()).unwrap()
}

fn app(sources: &[SourceSnapshot]) -> App {
    let mut app = App::offline("backend-fixture.seismograph".into());
    app.finish_offline_load(capture(sources));
    if let Screen::Offline { tab, .. } = &mut app.screen {
        *tab = MonitorTab::Heaps;
    }
    app
}

fn render(app: &App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
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
fn filtering_real_container_events_preserves_both_original_typed_backend_snapshots() {
    let native = crate::native_view::fixture::snapshot();
    let mimalloc = fixture::snapshot();
    let original = capture(&[fixture::source(&native.clone().into()), fixture::source(&mimalloc)]);
    let index = original.filter_index.as_ref().unwrap();
    let filter = FilterSpec::parse("crate:not_captured", "", false, RuntimeStackMode::Event).unwrap();

    let filtered = index.render(&filter);

    assert_eq!(
        (
            filtered.native.as_deref(),
            filtered.mimalloc.as_deref(),
            filtered.filter_summary.events.total,
            filtered.filter_summary.events.shown,
            filtered.heap_error,
            filtered.memory.is_some(),
        ),
        (Some(&native), Some(&mimalloc), 1, 0, None, false),
    );
    assert!(std::sync::Arc::ptr_eq(
        original.mimalloc.as_ref().unwrap(),
        filtered.mimalloc.as_ref().unwrap()
    ));
}

#[test]
fn source_backend_mismatch_stops_monitor_snapshot_preparation() {
    let mut source = fixture::source(&fixture::snapshot());
    source.id = seismograph_rallocator::source::ID;
    let decoded = seismograph::snapshot::decode(&fixture::container(&[source])).unwrap();

    let error = super::snapshot::prepare(decoded).err().unwrap();

    assert_eq!(error, "allocator source snmalloc contains a mimalloc payload");
}

#[test]
fn duplicate_mimalloc_sources_stop_monitor_preparation_instead_of_replacing_state() {
    let source = fixture::source(&fixture::snapshot());
    let mut decoded = seismograph::snapshot::decode(&fixture::container(std::slice::from_ref(&source))).unwrap();
    decoded.sources.push(source);

    let error = super::snapshot::prepare(decoded).err().unwrap();

    assert_eq!(error, "multiple mimalloc allocator sources make the inventory ambiguous");
}

#[test]
fn filtered_mimalloc_capture_renders_the_same_global_measurements() {
    let capture = capture(&[fixture::source(&fixture::snapshot())]);
    let filtered = capture
        .filter_index
        .as_ref()
        .unwrap()
        .render(&FilterSpec::parse("crate:not_captured", "", false, RuntimeStackMode::Event).unwrap());
    let mut app = App::offline("filtered-backend.seismograph".into());
    app.finish_offline_load(filtered);
    if let Screen::Offline { tab, .. } = &mut app.screen {
        *tab = MonitorTab::Heaps;
    }

    let text = render(&app, 180, 60);

    for expected in [
        "Mimalloc",
        "OS reservation: 10240 B",
        "Arena bitmap commitment: 4096 B",
        "Direct OS data committed: 1024 B",
        "Total OS commitment: UNKNOWN",
        "class 60: 2 INDEXED regular abandoned pages",
    ] {
        assert!(text.contains(expected), "missing {expected}\n{text}");
    }
    assert!(!text.contains("Native allocator source unavailable"));
}

#[test]
fn indexed_abandoned_zeroes_preserve_unknown_coverage_for_huge_singleton_and_os_pages() {
    let mut payload = fixture::snapshot();
    let seismograph_rallocator::snapshot::BackendState::Mimalloc(state) = &mut payload.backend else {
        unreachable!();
    };
    state.global.abandoned_pages.fill(0);
    let mut app = app(&[fixture::source(&payload)]);
    app.handle_key(KeyCode::Enter);
    app.handle_key(KeyCode::End);

    let text = render(&app, 180, 60);

    for expected in [
        "INDEXED regular abandoned pages by class (bins 0..60",
        "All INDEXED regular abandoned-page counts (bins 0..60) are zero",
        "Unindexed abandoned pages: UNKNOWN (huge/singleton pages and some OS-backed pages)",
        "Zeros do not prove their absence.",
    ] {
        assert!(text.contains(expected), "missing {expected}\n{text}");
    }
    assert!(!text.contains("All abandoned-index counts are zero"));
}

#[test]
fn narrow_global_details_expose_the_full_header_instead_of_only_clipped_summary_rows() {
    let mut app = app(&[fixture::source(&fixture::snapshot())]);
    app.handle_key(KeyCode::Enter);

    let text = render(&app, 70, 32);

    for expected in ["publication", "\n│enabled ", "capture 100 ns", "session 7", "round 5"] {
        assert!(text.contains(expected), "missing {expected}\n{text}");
    }
}

#[test]
fn keyboard_reaches_all_mimalloc_bins_and_returns_to_the_capture_root() {
    let mut app = app(&[fixture::source(&fixture::snapshot())]);
    for key in [KeyCode::Down, KeyCode::Enter, KeyCode::End, KeyCode::Enter] {
        assert!(!app.handle_key(key));
    }
    let details = render(&app, 180, 60);
    assert_eq!(
        (
            app.heap_view.mimalloc.selected_owner,
            app.heap_view.mimalloc.class,
            app.heap_view.mimalloc.depth
        ),
        (Some(11), 73, Depth::Detail),
    );
    assert!(details.contains("Heap #11") && details.contains("Class 73") && details.contains("Initialized capacity blocks: 2"));
    for key in [KeyCode::Backspace, KeyCode::Esc] {
        assert!(!app.handle_key(key));
    }
    assert_eq!(app.heap_view.mimalloc.depth, Depth::Root);
    assert!(matches!(app.screen, Screen::Offline { snapshot: Some(_), .. }));
}

#[test]
fn class_details_scroll_to_actual_wrapped_bounds_and_survive_resize() {
    let mut app = app(&[fixture::source(&fixture::snapshot())]);
    for key in [KeyCode::Down, KeyCode::Enter, KeyCode::Down, KeyCode::Enter] {
        app.handle_key(key);
    }
    render(&app, 80, 24);
    app.handle_key(KeyCode::End);
    let bottom = render(&app, 80, 24);
    let limit = app.panels.rows.native_scroll_limit.get();
    assert!(limit > 5 && bottom.contains("Retired pages: 1"));
    app.handle_key(KeyCode::PageUp);
    assert_ne!(render(&app, 80, 24), bottom);
    assert_eq!(app.heap_view.mimalloc.scroll, limit - 5);
    app.handle_key(KeyCode::End);
    render(&app, 80, 32);
    let resized_limit = app.panels.rows.native_scroll_limit.get();
    app.handle_key(KeyCode::Up);
    assert_eq!(app.heap_view.mimalloc.scroll, resized_limit.saturating_sub(1));
    app.handle_key(KeyCode::Home);
    let top = render(&app, 80, 32);
    assert!(top.contains("Heap #11") && top.contains("Page walk: PARTIAL"));
}

#[test]
fn unknown_heap_class_details_never_invent_measured_zeroes() {
    let mut app = app(&[fixture::source(&fixture::snapshot())]);
    for key in [KeyCode::Down, KeyCode::Down, KeyCode::Enter, KeyCode::Enter] {
        app.handle_key(key);
    }

    let text = render(&app, 180, 60);

    assert!(text.contains("Heap #12") && text.contains("UNOBSERVED") && text.contains("UNKNOWN, not zero"));
    assert!(!text.contains("Used blocks:") && !text.contains("Initialized capacity blocks:"));
}

#[test]
fn exited_heaps_show_historical_copies_and_never_a_current_lease() {
    let mut app = app(&[fixture::source(&fixture::snapshot())]);
    app.activate_mouse_row(ListTarget::HeapBuckets, 6);
    app.handle_key(KeyCode::Enter);
    app.activate_mouse_row(ListTarget::NativeClasses, 1);
    app.handle_key(KeyCode::Enter);

    let text = render(&app, 180, 60);

    assert!(text.contains("Heap #16") && text.contains("EXITED / historical observation") && text.contains("Used blocks: 40"));
    assert!(!text.contains("stale lease") && !text.contains("current lease"));
}

#[test]
fn backend_switching_keeps_both_explorers_and_their_independent_selections() {
    let mut app = app(&[
        fixture::source(&crate::native_view::fixture::snapshot().into()),
        fixture::source(&fixture::snapshot()),
    ]);
    app.handle_key(KeyCode::Down);
    app.handle_key(KeyCode::Down);
    app.handle_key(KeyCode::Enter);
    let native_selection = app.heap_view.native;
    app.handle_key(KeyCode::Char('b'));
    for key in [KeyCode::Down, KeyCode::Enter, KeyCode::End, KeyCode::Enter] {
        app.handle_key(key);
    }
    let mimalloc_selection = app.heap_view.mimalloc;
    let mimalloc_text = render(&app, 180, 60);
    assert!(mimalloc_text.contains("Backend: mimalloc") && mimalloc_text.contains("Heap #11"));
    app.handle_key(KeyCode::Char('b'));
    let native_text = render(&app, 180, 60);
    assert!(native_text.contains("Backend: snmalloc") && native_text.contains("Small classes"));
    app.handle_key(KeyCode::Char('b'));

    assert_eq!(
        (app.heap_view.backend, app.heap_view.native, app.heap_view.mimalloc),
        (AllocatorBackend::Mimalloc, native_selection, mimalloc_selection),
    );
}

#[test]
fn snmalloc_capture_replaces_a_remembered_mimalloc_backend_selection() {
    let mut app = app(&[fixture::source(&fixture::snapshot())]);
    assert_eq!(app.heap_view.backend, AllocatorBackend::Mimalloc);
    let native = fixture::source(&crate::native_view::fixture::snapshot().into());

    app.finish_offline_load(capture(&[native]));

    assert_eq!(app.heap_view.backend, AllocatorBackend::Snmalloc);
}

#[test]
fn missing_selected_heap_resets_navigation_instead_of_retargeting_another_lifetime() {
    let payload = fixture::snapshot();
    let (_, state) = crate::mimalloc_view::parts(&payload).unwrap();
    let mut nav = super::mimalloc_ui::Navigation::default();
    for key in [KeyCode::Down, KeyCode::Enter, KeyCode::End, KeyCode::Enter] {
        nav.key(key, Some(state));
    }
    let mut replacement = state.clone();
    replacement.owners.remove(0);

    nav.reconcile(&replacement);

    assert_eq!(nav, super::mimalloc_ui::Navigation::default());
}

#[test]
fn mimalloc_context_help_explains_bins_without_snmalloc_slab_semantics() {
    let mut app = app(&[fixture::source(&fixture::snapshot())]);
    for key in [KeyCode::Down, KeyCode::Enter, KeyCode::F(1)] {
        app.handle_key(key);
    }

    let text = render(&app, 180, 60);

    for expected in ["Mimalloc page classes", "pending remote frees", "Reserved / Capacity blocks"] {
        assert!(text.contains(expected), "missing {expected}\n{text}");
    }
    assert!(!text.contains("Small classes and slab geometry"));
}
