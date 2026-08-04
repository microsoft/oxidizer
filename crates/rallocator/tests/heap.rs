use std::alloc::{Layout, alloc, dealloc};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, mpsc};

use allocation_hints::domain::Domain;
use allocation_hints::heap::bump::Options as BumpOptions;
use allocation_hints::heap::general::Options as GeneralOptions;
use allocation_hints::heap::{Heap, InfoKind, Kind, Options, UsageKind, thread_heap};
use allocation_hints::{Hint, with_hint};

rallocator::rallocator!();

#[test]
fn hints_compare_and_format_by_heap_identity() {
    let first = Heap::new();
    let second = Heap::new();

    assert_eq!(Hint::new(), Hint::global());
    assert_eq!(Hint::new().with_heap(&first), Hint::new().with_heap(&first));
    assert_ne!(Hint::new().with_heap(&first), Hint::new().with_heap(&second));
    assert_ne!(Hint::new(), Hint::new().with_heap(&first));
    assert!(format!("{:?}", Hint::new().with_heap(&first)).contains("Hint"));
}

#[test]
fn domains_expose_debug_identity() {
    let domain = Domain::new();
    let debug = format!("{domain:?}");
    assert!(debug.contains("Domain"));
    assert!(debug.contains("identity"));
}

#[test]
fn public_heap_metadata_and_usage_accessors_cover_both_heap_kinds() {
    let general_options = GeneralOptions::default();
    let general = Heap::default();
    assert!(matches!(Options::default().kind(), Kind::General(_)));
    assert_eq!(general.as_ref().info().domain(), Domain::default());
    assert!(!general.info().is_active());
    let general_info = general.info();
    let InfoKind::General(info) = general_info.kind() else {
        panic!("expected general heap info");
    };
    assert_eq!(info.options(), general_options);
    assert!(!info.is_thread_target());
    assert!(format!("{general:?}").contains("Heap"));

    let usage = general.usage().unwrap();
    assert!(matches!(usage.kind(), UsageKind::General(_)));
    assert!(usage.general().is_some());
    assert!(usage.bump().is_none());
    assert!(usage.is_empty());
    assert_eq!(usage.live_allocations(), 0);
    assert_eq!(usage.live_requested_bytes(), 0);
    assert_eq!(usage.live_usable_bytes(), 0);
    let general_usage = usage.general().unwrap();
    assert_eq!(general_usage.small().live_allocations(), 0);
    assert_eq!(general_usage.small().requested_bytes(), 0);
    assert_eq!(general_usage.small().usable_bytes(), 0);
    assert_eq!(general_usage.medium().live_allocations(), 0);
    assert_eq!(general_usage.direct().live_allocations(), 0);
    assert_eq!(general_usage.cached_medium_bytes(), 0);
    assert_eq!(general_usage.slab_count(), 0);
    assert_eq!(general_usage.slice_count(), 0);
    assert!(usage.reserved_bytes() >= usage.committed_bytes());

    let bump_options = BumpOptions::default();
    let bump = Heap::with_options(Options::bump(bump_options));
    let bump_info = bump.info();
    let InfoKind::Bump(info) = bump_info.kind() else {
        panic!("expected bump heap info");
    };
    assert_eq!(info.options(), bump_options);
    let usage = bump.usage().unwrap();
    assert!(matches!(usage.kind(), UsageKind::Bump(_)));
    assert!(usage.general().is_none());
    let bump_usage = usage.bump().unwrap();
    assert_eq!(bump_usage.cursor_used_bytes(), 0);
    assert_eq!(bump_usage.allocation_count(), 0);
    assert_eq!(bump_usage.chunk_count(), 1);
}

#[test]
fn heaps_use_the_default_domain_unless_one_is_selected() {
    let default_domain = Domain::default();
    let default_heap = Heap::new();
    let private_domain = Domain::new();
    let private_heap = Heap::with_options(Options::default().with_domain(private_domain));

    assert_eq!(default_heap.info().domain(), default_domain);
    assert_eq!(private_heap.info().domain(), private_domain);
    assert_ne!(private_heap.info().domain(), default_heap.info().domain());
}

#[test]
fn domains_isolate_region_backing_between_heaps() {
    let first_domain = Domain::new();
    let second_domain = Domain::new();
    let options = GeneralOptions::new().with_medium_cache_max_bytes(0);
    let first_heap = Heap::with_options(Options::general(options).with_domain(first_domain));
    let second_heap = Heap::with_options(Options::general(options).with_domain(second_domain));
    let layout = Layout::from_size_align(128 * 1024, 16).unwrap();

    let first_address = with_hint(Hint::new().with_heap(&first_heap), || unsafe {
        let address = alloc(layout);
        assert!(!address.is_null());
        dealloc(address, layout);
        address
    });
    let second_address = with_hint(Hint::new().with_heap(&second_heap), || unsafe {
        let address = alloc(layout);
        assert!(!address.is_null());
        dealloc(address, layout);
        address
    });
    let reused_first_address = with_hint(Hint::new().with_heap(&first_heap), || unsafe {
        let address = alloc(layout);
        assert!(!address.is_null());
        dealloc(address, layout);
        address
    });

    assert_ne!(first_address, second_address);
    assert_eq!(reused_first_address, first_address);
}

#[test]
fn custom_general_options_use_the_general_allocation_path() {
    let options = GeneralOptions::new()
        .with_locality_segment_bytes(64 * 1024)
        .with_medium_cache_max_bytes(0);
    let heap = Heap::with_options(Options::general(options));

    let values = with_hint(Hint::new().with_heap(&heap), || {
        let small = Box::new([1_u8; 64]);
        let medium = vec![2_u8; 128 * 1024];
        (small, medium)
    });

    assert_eq!(options.locality_segment_bytes(), 64 * 1024);
    assert_eq!(options.medium_cache_max_bytes(), 0);
    assert!(heap.usage().unwrap().general().is_some());
    assert_eq!(heap.usage().unwrap().live_allocations(), 2);
    assert_eq!(values.0[0], 1);
    assert_eq!(values.1[0], 2);
}

#[test]
fn general_heap_info_is_cheap_and_reports_activation() {
    let options = GeneralOptions::new()
        .with_locality_segment_bytes(64 * 1024)
        .with_medium_cache_max_bytes(0);
    let heap = Heap::with_options(Options::general(options));

    let info = heap.info();
    assert!(!info.is_active());
    let InfoKind::General(general) = info.kind() else {
        panic!("expected general heap info");
    };
    assert_eq!(general.options(), options);
    assert!(!general.is_thread_target());

    with_hint(Hint::new().with_heap(&heap), || {
        assert!(heap.info().is_active());
    });
    assert!(!heap.info().is_active());
}

#[test]
fn general_usage_reports_small_medium_and_direct_allocations() {
    let heap = Heap::with_options(Options::general(GeneralOptions::new().with_medium_cache_max_bytes(0)));
    let small_layout = Layout::from_size_align(64, 16).unwrap();
    let medium_layout = Layout::from_size_align(128 * 1024, 16).unwrap();
    let direct_layout = Layout::from_size_align(64, 128 * 1024).unwrap();

    let (small, medium, direct) = with_hint(Hint::new().with_heap(&heap), || unsafe {
        let small = alloc(small_layout);
        let medium = alloc(medium_layout);
        let direct = alloc(direct_layout);
        assert!(!small.is_null());
        assert!(!medium.is_null());
        assert!(!direct.is_null());

        let usage = heap.usage().unwrap();
        let general = usage.general().unwrap();
        assert_eq!(usage.live_allocations(), 3);
        assert_eq!(usage.live_requested_bytes(), 128 * 1024 + 128);
        assert_eq!(general.small().live_allocations(), 1);
        assert_eq!(general.small().requested_bytes(), 64);
        assert_eq!(general.small().usable_bytes(), 64);
        assert_eq!(general.medium().live_allocations(), 1);
        assert_eq!(general.medium().requested_bytes(), 128 * 1024);
        assert_eq!(general.medium().usable_bytes(), 128 * 1024);
        assert_eq!(general.direct().live_allocations(), 1);
        assert_eq!(general.direct().requested_bytes(), 64);
        (small, medium, direct)
    });

    unsafe {
        dealloc(small, small_layout);
        dealloc(medium, medium_layout);
        dealloc(direct, direct_layout);
    }
    assert!(heap.usage().unwrap().is_empty());
}

#[test]
fn general_usage_reports_cached_medium_spans() {
    let heap = Heap::new();
    let layout = Layout::from_size_align(128 * 1024, 16).unwrap();
    with_hint(Hint::new().with_heap(&heap), || unsafe {
        let address = alloc(layout);
        dealloc(address, layout);
    });

    let usage = heap.usage().unwrap();
    let general = usage.general().unwrap();
    assert!(usage.is_empty());
    assert_eq!(general.cached_medium_bytes(), 128 * 1024);
}

#[test]
fn thread_heap_usage_requires_its_owner_thread() {
    let heap = thread_heap().unwrap();
    let error = std::thread::spawn(move || heap.usage().unwrap_err()).join().unwrap();
    assert_eq!(error.to_string(), "thread-heap usage must be queried from its owner thread");
    assert!(format!("{error:?}").starts_with("Error("));
}

#[test]
fn thread_heap_usage_includes_remote_allocations() {
    let heap = thread_heap().unwrap();
    let before = heap.usage().unwrap();
    let remote_heap = thread_heap().unwrap();
    let value = std::thread::spawn(move || with_hint(Hint::new().with_heap(&remote_heap), || Box::new([7_u8; 64])))
        .join()
        .unwrap();

    let during = heap.usage().unwrap();
    assert_eq!(during.live_allocations(), before.live_allocations() + 1);
    assert_eq!(during.live_requested_bytes(), before.live_requested_bytes() + 64);
    assert!(during.general().unwrap().slice_count() > before.general().unwrap().slice_count());
    drop(value);
    assert_eq!(heap.usage().unwrap().live_allocations(), before.live_allocations());
}

#[test]
fn thread_heap_usage_is_consistent_during_remote_medium_free() {
    let heap = thread_heap().unwrap();
    let value = Vec::<u8>::with_capacity(128 * 1024);
    assert_eq!(heap.usage().unwrap().general().unwrap().medium().live_allocations(), 1);

    let done = Arc::new(AtomicBool::new(false));
    let worker_done = Arc::clone(&done);
    let worker = std::thread::spawn(move || {
        drop(value);
        worker_done.store(true, Ordering::Release);
    });
    while !done.load(Ordering::Acquire) {
        assert!(heap.usage().unwrap().general().unwrap().medium().live_allocations() <= 1);
    }

    worker.join().unwrap();
    assert_eq!(heap.usage().unwrap().general().unwrap().medium().live_allocations(), 0);
}

#[test]
fn thread_heap_usage_includes_medium_allocated_before_handle_creation() {
    let value = Vec::<u8>::with_capacity(128 * 1024);
    let heap = thread_heap().unwrap();
    assert_eq!(heap.usage().unwrap().general().unwrap().medium().live_allocations(), 1);
    drop(value);
}

#[test]
fn general_options_reject_unsupported_cache_cutoffs() {
    assert!(
        std::panic::catch_unwind(|| {
            GeneralOptions::new().with_medium_cache_max_bytes(96 * 1024);
        })
        .is_err()
    );
    assert!(
        std::panic::catch_unwind(|| {
            GeneralOptions::new().with_locality_segment_bytes(96 * 1024);
        })
        .is_err()
    );
}

#[test]
fn explicit_general_heap_reuses_blocks_and_outlives_its_handle() {
    let heap = Heap::new();
    let hint = Hint::new().with_heap(&heap);
    drop(heap);

    let value = with_hint(hint, || {
        let first = Box::new([1_u8; 64]);
        let first_address = (&raw const *first).addr();
        drop(first);

        let second = Box::new([2_u8; 64]);
        assert_eq!((&raw const *second).addr(), first_address);
        second
    });

    assert_eq!(value[0], 2);
    std::thread::spawn(move || drop(value)).join().unwrap();
}

#[test]
fn global_hint_bypasses_an_explicit_general_heap() {
    let heap = Heap::new();
    let (explicit, global) = with_hint(Hint::new().with_heap(&heap), || {
        let explicit = Box::new([1_u8; 64]);
        let global = with_hint(Hint::global(), || Box::new([2_u8; 64]));
        (explicit, global)
    });

    let explicit_slab = (&raw const *explicit).addr() & !((32 * 1024) - 1);
    let global_slab = (&raw const *global).addr() & !((32 * 1024) - 1);
    assert_ne!(explicit_slab, global_slab);
}

#[test]
fn nested_heap_scopes_restore_the_previous_heap() {
    let outer = Heap::new();
    let inner = Heap::new();

    let (outer_first, inner_value, outer_second) = with_hint(Hint::new().with_heap(&outer), || {
        let outer_first = Box::new([1_u8; 64]);
        let inner_value = with_hint(Hint::new().with_heap(&inner), || Box::new([2_u8; 64]));
        let outer_second = Box::new([3_u8; 64]);
        (outer_first, inner_value, outer_second)
    });

    let outer_first_slab = (&raw const *outer_first).addr() & !((32 * 1024) - 1);
    let outer_second_slab = (&raw const *outer_second).addr() & !((32 * 1024) - 1);
    let inner_slab = (&raw const *inner_value).addr() & !((32 * 1024) - 1);
    assert_eq!(outer_first_slab, outer_second_slab);
    assert_ne!(outer_first_slab, inner_slab);
}

#[test]
fn a_heap_can_be_reentered_around_another_heap() {
    let outer = Heap::new();
    let inner = Heap::new();

    with_hint(Hint::new().with_heap(&outer), || {
        with_hint(Hint::new().with_heap(&inner), || {
            with_hint(Hint::new().with_heap(&outer), || {
                let value = Box::new([1_u8; 64]);
                assert_eq!(value[0], 1);
            });
        });
    });
}

#[test]
fn general_heap_can_migrate_between_threads() {
    let heap = Heap::new();
    let first_address = with_hint(Hint::new().with_heap(&heap), || {
        let value = Box::new([1_u8; 64]);
        let address = (&raw const *value).addr();
        drop(value);
        address
    });

    let (heap, value_address) = std::thread::spawn(move || {
        let value = with_hint(Hint::new().with_heap(&heap), || Box::new([2_u8; 64]));
        let address = (&raw const *value).addr();
        drop(value);
        (heap, address)
    })
    .join()
    .unwrap();

    assert_eq!(value_address, first_address);
    drop(heap);
}

#[test]
fn general_heap_rejects_simultaneous_activation() {
    let heap = Heap::new();
    let entered = Arc::new(Barrier::new(2));
    let leave = Arc::new(Barrier::new(2));
    let worker_hint = Hint::new().with_heap(&heap);
    let worker_entered = Arc::clone(&entered);
    let worker_leave = Arc::clone(&leave);

    let worker = std::thread::spawn(move || {
        with_hint(worker_hint, || {
            worker_entered.wait();
            worker_leave.wait();
        });
    });

    entered.wait();
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        with_hint(Hint::new().with_heap(&heap), || {});
    }));
    leave.wait();
    worker.join().unwrap();

    assert!(result.is_err());
}

#[test]
fn general_usage_waits_for_another_threads_active_scope() {
    let heap = Heap::new();
    let worker_hint = Hint::new().with_heap(&heap);
    let (entered_tx, entered_rx) = mpsc::sync_channel(0);
    let (leave_tx, leave_rx) = mpsc::sync_channel(0);
    let worker = std::thread::spawn(move || {
        with_hint(worker_hint, || {
            entered_tx.send(()).unwrap();
            leave_rx.recv().unwrap();
        });
    });
    entered_rx.recv().unwrap();

    let (usage_tx, usage_rx) = mpsc::sync_channel(0);
    let (query_started_tx, query_started_rx) = mpsc::sync_channel(0);
    let query = std::thread::spawn(move || {
        query_started_tx.send(()).unwrap();
        usage_tx.send(heap.usage().unwrap()).unwrap();
    });
    query_started_rx.recv().unwrap();
    assert!(usage_rx.try_recv().is_err());
    leave_tx.send(()).unwrap();
    let _usage = usage_rx.recv().unwrap();

    worker.join().unwrap();
    query.join().unwrap();
}

#[test]
fn general_usage_remains_consistent_during_remote_frees() {
    let heap = Heap::new();
    let allocation_count = if cfg!(miri) { 32 } else { 1_000 };
    let mut values = Vec::with_capacity(allocation_count);
    with_hint(Hint::new().with_heap(&heap), || {
        for _ in 0..allocation_count {
            values.push(Box::new([1_u8; 64]));
        }
    });

    let done = Arc::new(AtomicBool::new(false));
    let worker_done = Arc::clone(&done);
    let worker = std::thread::spawn(move || {
        drop(values);
        worker_done.store(true, Ordering::Release);
    });
    while !done.load(Ordering::Acquire) {
        let usage = heap.usage().unwrap();
        assert!(usage.live_allocations() <= allocation_count);
        assert!(usage.live_requested_bytes() <= allocation_count * 64);
    }
    worker.join().unwrap();
    assert!(heap.usage().unwrap().is_empty());
}

#[test]
fn thread_heap_handle_uses_the_local_fast_path_on_its_owner() {
    let heap = thread_heap().unwrap();
    assert!(!heap.info().is_active());
    let first_address = with_hint(Hint::new().with_heap(&heap), || {
        assert!(heap.info().is_active());
        let value = Box::new([1_u8; 64]);
        let address = (&raw const *value).addr();
        drop(value);
        address
    });
    assert!(!heap.info().is_active());

    let value = Box::new([2_u8; 64]);
    assert_eq!((&raw const *value).addr(), first_address);
}

#[test]
fn remote_allocation_returns_to_the_owner_thread_heap() {
    let heap = thread_heap().unwrap();
    let layout = Layout::from_size_align(64, 16).unwrap();
    let address = std::thread::spawn(move || {
        with_hint(Hint::new().with_heap(&heap), || {
            let address = unsafe { alloc(layout) };
            assert!(!address.is_null());
            address.addr()
        })
    })
    .join()
    .unwrap();

    unsafe { dealloc(address as *mut u8, layout) };
    let reused = unsafe { alloc(layout) };
    assert_eq!(reused.addr(), address);
    unsafe { dealloc(reused, layout) };
}

#[test]
fn remote_thread_heap_handles_support_concurrent_producers() {
    let handles = (0..8).map(|_| thread_heap().unwrap()).collect::<Vec<_>>();
    let workers = handles
        .into_iter()
        .enumerate()
        .map(|(worker, heap)| {
            std::thread::spawn(move || {
                with_hint(Hint::new().with_heap(&heap), || {
                    (0..1_000).map(|index| Box::new([worker as u64, index])).collect::<Vec<_>>()
                })
            })
        })
        .collect::<Vec<_>>();

    for (worker, values) in workers.into_iter().enumerate() {
        let values = values.join().unwrap();
        assert_eq!(values.len(), 1_000);
        assert_eq!(values[999][0], worker as u64);
        assert_eq!(values[999][1], 999);
    }
}

#[test]
fn thread_heap_handle_remains_memory_safe_after_owner_exit() {
    let heap = std::thread::spawn(|| thread_heap().unwrap()).join().unwrap();
    let value = with_hint(Hint::new().with_heap(&heap), || Box::new([7_u8; 64]));
    assert_eq!(value[0], 7);
    drop(value);
    assert_eq!(
        heap.usage().unwrap_err().to_string(),
        "thread-heap usage must be queried from its owner thread"
    );
    let heap_info = heap.info();
    let InfoKind::General(info) = heap_info.kind() else {
        panic!("expected general heap info");
    };
    assert!(info.is_thread_target());
}
