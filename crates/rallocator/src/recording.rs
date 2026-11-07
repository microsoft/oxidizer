// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Stateless event bridge. The recorder owns policy and suppression.

use std::alloc::Layout;

use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind};
use seismograph::recorder::event::{Address, EventClass, Record};
use seismograph::recorder::{is_enabled, without_recording};

static SOURCE: seismograph::snapshot::Source = seismograph::snapshot::Source::new(
    seismograph_rallocator::source::ID,
    seismograph_rallocator::source::NAME,
    seismograph_rallocator::source::SCHEMA_VERSION,
    capture,
);

pub(crate) fn register_source() {
    // Owner creation already holds the pool lock. Source registration performs
    // only atomic linkage and cannot reenter the allocator.
    seismograph::snapshot::register_source(&SOURCE);
}

#[inline]
pub(crate) fn allocate(layout: Layout, zeroed: bool) -> *mut u8 {
    // Keep the recorder's closure environment and register saves off the
    // disabled path; the enabled body still uses the lazy record closure.
    dispatch_recording(
        is_enabled(EventClass::Allocation),
        || allocate_recorded(layout, zeroed),
        || allocate_core(layout, zeroed),
    )
}

#[inline]
fn dispatch_recording<T>(enabled: bool, recorded: impl FnOnce() -> T, plain: impl FnOnce() -> T) -> T {
    if enabled { recorded() } else { plain() }
}

#[inline]
fn allocate_core(layout: Layout, zeroed: bool) -> *mut u8 {
    if zeroed {
        crate::thread::allocate_zeroed(layout)
    } else {
        crate::thread::allocate(layout)
    }
}

#[inline(never)]
fn allocate_recorded(layout: Layout, zeroed: bool) -> *mut u8 {
    allocate_recorded_with(layout, zeroed, emit)
}

fn allocate_recorded_with(layout: Layout, zeroed: bool, emit: impl FnOnce(*mut u8, usize, usize, bool)) -> *mut u8 {
    let address = allocate_core(layout, zeroed);
    if !address.is_null() {
        emit(address, layout.size(), layout.align(), false);
        crate::observation::publish();
    }
    address
}

/// # Safety
/// `address` is a live allocation with `layout` from this allocator.
#[inline]
pub(crate) unsafe fn deallocate(address: *mut u8, layout: Layout) {
    dispatch_recording(
        is_enabled(EventClass::Allocation),
        || {
            // SAFETY: The caller transfers this allocation once through the selected branch.
            unsafe { deallocation_recorded(address, layout) };
        },
        || {
            // SAFETY: Only this branch consumes the caller's live allocation when recording is disabled.
            unsafe { crate::thread::deallocate(address) };
        },
    );
}

#[inline(never)]
unsafe fn deallocation_recorded(address: *mut u8, layout: Layout) {
    // #[gamma::skip(call_result.default, reason = "emit still executes and returns unit, whose default is the identical unit value.")]
    emit(address, layout.size(), layout.align(), true);
    // SAFETY: The caller transfers a live allocation for destruction.
    unsafe { crate::thread::deallocate(address) };
    crate::observation::publish();
}

/// # Safety
/// `address` is live for `layout`, and `new_size` satisfies `GlobalAlloc`'s contract.
#[inline]
pub(crate) unsafe fn reallocate(address: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    dispatch_recording(
        is_enabled(EventClass::Allocation),
        || {
            // SAFETY: The selected branch forwards the caller's live allocation and resize contract.
            unsafe { reallocate_recorded(address, layout, new_size) }
        },
        || {
            // SAFETY: The disabled path receives the same exclusive allocation and resize bound.
            unsafe { crate::reallocate_core(address, layout, new_size) }
        },
    )
}

#[inline(never)]
unsafe fn reallocate_recorded(address: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    let release = |pointer| {
        // SAFETY: Successful replacement transfers the still-live old allocation.
        unsafe { crate::thread::deallocate(pointer) };
    };
    // SAFETY: The caller supplies a live allocation, retired once by the callback.
    unsafe { reallocate_recorded_with_release(address, layout, new_size, release) }
}

unsafe fn reallocate_recorded_with_release(address: *mut u8, layout: Layout, new_size: usize, release: impl FnOnce(*mut u8)) -> *mut u8 {
    let Some(request) = crate::classes::Request::with_size(layout, new_size) else {
        return std::ptr::null_mut();
    };
    let retained = crate::classes::Request::new(layout) == Some(request);
    let replacement = if retained {
        address
    } else {
        // Match the core's acquisition of a possibly cross-thread pointer before
        // copying, but keep the original live until recording leaves its borrow.
        let replacement = allocate_acquired_with(request, crate::thread::allocate_request);
        if replacement.is_null() {
            return replacement;
        }
        // SAFETY: Both allocations are live, disjoint, and cover the copy bound.
        unsafe { std::ptr::copy_nonoverlapping(address, replacement, layout.size().min(new_size)) };
        replacement
    };
    // #[gamma::skip(logical.and_remove_left, reason = "The retained pointer is nonnull by the live-allocation contract; a newly allocated null replacement already returns above")]
    if !replacement.is_null() && new_size != layout.size() {
        record_successful_resize(
            (address, layout),
            (replacement, new_size),
            retained,
            emit,
            release,
            crate::observation::publish,
        );
    }
    replacement
}

fn allocate_acquired_with<T>(request: T, allocate: impl FnOnce(T) -> *mut u8) -> *mut u8 {
    crate::sync_fence::fence(std::sync::atomic::Ordering::Acquire);
    allocate(request)
}

fn record_successful_resize(
    allocation: (*mut u8, Layout),
    replacement: (*mut u8, usize),
    retained: bool,
    mut emit: impl FnMut(*mut u8, usize, usize, bool),
    release: impl FnOnce(*mut u8),
    publish: impl FnOnce(),
) {
    let (address, layout) = allocation;
    emit(address, layout.size(), layout.align(), true);
    if !retained {
        release(address);
    }
    emit(replacement.0, replacement.1, layout.align(), false);
    publish();
}

#[inline]
fn emit(address: *mut u8, size: usize, alignment: usize, freed: bool) {
    seismograph::record(EventClass::Allocation, || {
        if address.is_null() {
            return None;
        }
        let allocation = Allocation {
            // This is an address correlation key, not a unique lifetime ID.
            allocation_id: AllocationId::new(address.addr() as u64),
            // The container already records the actor. Zero means no separate
            // allocator thread or heap identity is available.
            event_thread_id: EventThreadId::new(0),
            heap_id: HeapId::new(0),
            heap_kind: HeapKind::General,
            freed_after_heap_release: false,
            address: Address::from_ptr(address),
            size: size as u64,
            alignment: alignment as u64,
        };
        Some(if freed {
            Record::deallocation(allocation)
        } else {
            Record::allocation(allocation)
        })
    });
}

fn capture(_context: seismograph::snapshot::SnapshotContext<'_>) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    capture_with_inventory(crate::thread::inventory)
}

fn capture_with_inventory(
    inventory: impl FnOnce(u64, u64, usize) -> Result<(crate::observation::OwnerBuffer, u64, bool), seismograph::Error>,
) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    without_recording(|| {
        let round = seismograph_rallocator::native::observation_round();
        let captured_nanos = seismograph_rallocator::native::captured_nanos();
        let (owners, owner_count, owners_complete) = inventory(round, captured_nanos, crate::observation::WALK_BUDGET)?;
        // Pool collection has ended before taking the global backend lock; owner
        // creation takes pool -> backend, so the collector must not reverse that order.
        let global = crate::backend::observe();
        let snapshot = seismograph_rallocator::native::Snapshot {
            captured_nanos: seismograph_rallocator::native::captured_nanos(),
            session_id: 0,
            round,
            owner_count,
            owners_complete,
            publication_enabled: seismograph_rallocator::native::publication_enabled(),
            owners: Vec::new(),
            global,
        };
        finish_capture_with(
            || {
                encode_capture_with(
                    || seismograph_rallocator::encoded_len_with_owners(&snapshot, owners.as_slice()),
                    seismograph::snapshot::SourceData::zeroed,
                    |bytes| seismograph_rallocator::encode_with_owners(&snapshot, owners.as_slice(), bytes),
                )
            },
            || {
                seismograph_rallocator::native::request_observation();
            },
        )
    })
}

fn finish_capture_with(
    encode: impl FnOnce() -> Result<seismograph::snapshot::SourceData, seismograph::Error>,
    request: impl FnOnce(),
) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let data = encode()?;
    request();
    Ok(data)
}

fn encode_capture_with<E>(
    size: impl FnOnce() -> Result<usize, E>,
    allocate: impl FnOnce(usize) -> Result<seismograph::snapshot::SourceData, seismograph::Error>,
    encode: impl FnOnce(&mut [u8]) -> Result<(), E>,
) -> Result<seismograph::snapshot::SourceData, seismograph::Error> {
    let len = size().map_err(|_error| seismograph::Error::new("rallocator source sizing failed"))?;
    let mut data = allocate(len)?;
    encode(data.as_mut_bytes()).map_err(|_error| seismograph::Error::new("rallocator source encoding failed"))?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use seismograph::recorder::event::Event;
    use seismograph::recorder::{Configuration, EventSampling, Policy};

    use super::*;

    static RECORDING: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Restore(bool);

    impl Drop for Restore {
        fn drop(&mut self) {
            seismograph::recorder(Configuration::default());
            seismograph_rallocator::native::set_publication_enabled(self.0);
        }
    }

    #[test]
    fn rejected_recorded_allocation_does_not_emit_an_allocation_event() {
        let layout = Layout::from_size_align(isize::MAX as usize, 1).unwrap();
        let address = allocate_recorded_with(layout, false, |_, _, _, _| {
            panic!("a rejected allocation must not emit a successful allocation");
        });
        assert!(address.is_null());
    }

    #[test]
    fn disabled_allocation_recording_does_not_publish_a_requested_owner_round() {
        let _lock = RECORDING.lock().unwrap();
        let _restore = Restore(seismograph_rallocator::native::publication_enabled());
        seismograph::recorder(Configuration::default());
        seismograph_rallocator::native::set_publication_enabled(true);
        let layout = Layout::from_size_align(64, 16).unwrap();
        let address = allocate_core(layout, false);
        assert!(!address.is_null());
        seismograph_rallocator::native::request_observation();
        let round = seismograph_rallocator::native::observation_round();
        crate::observation::publish();
        let mut observed = false;
        crate::thread::observe_current(|owner| {
            observed = true;
            assert_ne!(owner.observation().published().1.map(|observation| observation.round), Some(round));
        });
        // SAFETY: This test uniquely owns the allocation with its original layout.
        unsafe { crate::thread::deallocate(address) };
        assert!(observed);
    }

    #[test]
    fn null_allocation_emission_is_omitted_while_valid_emission_is_retained() {
        let _lock = RECORDING.lock().unwrap();
        let _restore = Restore(seismograph_rallocator::native::publication_enabled());
        seismograph::recorder(Configuration {
            allocations: Policy::all(false),
            ..Configuration::default()
        });
        let thread = seismograph::recorder::thread_id();
        let mut value = 0_u8;
        let address = &raw mut value;

        emit(address, 1, 1, false);
        emit(std::ptr::null_mut(), 1, 1, false);
        let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
        let allocations = decoded
            .events
            .events
            .iter()
            .filter(|event| event.thread_id == thread)
            .filter_map(Event::allocation)
            .filter(|allocation| allocation.address.get() == address.addr() as u64 || allocation.address.get() == 0)
            .map(|allocation| (allocation.address.get(), allocation.size, allocation.alignment))
            .collect::<Vec<_>>();

        assert_eq!(allocations, [(address.addr() as u64, 1, 1)]);
    }

    #[test]
    fn sampled_out_allocation_still_publishes_the_requested_native_round() {
        let _lock = RECORDING.lock().unwrap();
        let _restore = Restore(seismograph_rallocator::native::publication_enabled());
        seismograph_rallocator::native::set_publication_enabled(true);
        seismograph::recorder(Configuration {
            allocations: Policy {
                event_sampling: EventSampling::one_in(2).unwrap(),
                ..Policy::all(false)
            },
            ..Configuration::default()
        });
        seismograph_rallocator::native::request_observation();
        let requested = seismograph_rallocator::native::observation_round();
        let layout = Layout::from_size_align(64, 16).unwrap();

        // Fix only the sampling identity. The native operation and publication
        // use the production bridge, without depending on the allocator's address.
        let pointer = allocate_recorded_with(layout, false, |address, size, alignment, _freed| {
            seismograph::record(EventClass::Allocation, || {
                Some(Record::allocation(Allocation {
                    allocation_id: AllocationId::new(1),
                    event_thread_id: EventThreadId::new(0),
                    heap_id: HeapId::new(0),
                    heap_kind: HeapKind::General,
                    freed_after_heap_release: false,
                    address: Address::from_ptr(address),
                    size: size as u64,
                    alignment: alignment as u64,
                }))
            });
        });
        assert!(!pointer.is_null());
        let mut published = None;
        crate::thread::observe_current(|owner| {
            published = owner.observation().published().1.map(|observation| observation.round);
        });
        let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
        let events = decoded
            .events
            .events
            .iter()
            .filter(|event| {
                event
                    .allocation()
                    .is_some_and(|allocation| allocation.address.get() == pointer.addr() as u64)
            })
            .count();
        // SAFETY: The test uniquely owns the live allocation with its original layout.
        unsafe { deallocate(pointer, layout) };

        assert_eq!((events, published), (0, Some(requested)));
    }

    #[test]
    fn gamma_nov4_replacement_acquires_before_the_allocator_can_observe_the_request() {
        let trace = crate::sync_fence::trace::Trace::start();
        let mut byte = 17u8;
        let pointer = &raw mut byte;
        assert_eq!(
            allocate_acquired_with((71, 83), |request| {
                assert_eq!(request, (71, 83));
                assert_eq!(trace.events(), [std::sync::atomic::Ordering::Acquire]);
                pointer
            }),
            pointer
        );
        assert_eq!(trace.events(), [std::sync::atomic::Ordering::Acquire]);
    }

    #[test]
    fn gamma_nov4_disabled_dispatch_does_not_follow_a_later_recording_transition() {
        for operation in ["allocate", "deallocate", "reallocate"] {
            for enabled in [false, true] {
                let events = std::cell::RefCell::new(Vec::new());
                let active = std::cell::Cell::new(enabled);
                let result = dispatch_recording(
                    active.get(),
                    || {
                        assert!(active.get(), "{operation} must honor the gate observed before dispatch");
                        events.borrow_mut().push("recorded");
                        71
                    },
                    || {
                        active.set(true);
                        events.borrow_mut().push("plain-before-later-session");
                        83
                    },
                );
                assert_eq!(result, if enabled { 71 } else { 83 });
                assert_eq!(*events.borrow(), [if enabled { "recorded" } else { "plain-before-later-session" }]);
            }
        }
    }

    #[test]
    fn gamma_nov4_capture_completion_propagates_error_before_requesting_the_next_round() {
        let requested = std::cell::Cell::new(0);
        let result = finish_capture_with(
            || Err(seismograph::Error::new("precise capture failure")),
            || requested.set(requested.get() + 1),
        );
        assert!(result.err().unwrap().to_string().contains("precise capture failure"));
        assert_eq!(requested.get(), 0);
        let mut data = finish_capture_with(
            || seismograph::snapshot::SourceData::zeroed(17),
            || requested.set(requested.get() + 1),
        )
        .unwrap();
        assert_eq!(data.as_mut_bytes(), &[0; 17]);
        assert_eq!(requested.get(), 1);
    }

    #[test]
    fn gamma_nov4_unzeroed_entrypoints_preserve_known_initialized_reusable_tails() {
        use std::alloc::GlobalAlloc as _;

        without_recording(|| {
            let layout = Layout::from_size_align(33, 16).unwrap();
            for public_entrypoint in [false, true] {
                let (reusable, held, _) = gamma_survivor_prime_destination(layout);
                let pointer = if public_entrypoint {
                    // SAFETY: layout is a valid nonzero allocation layout.
                    unsafe { crate::Rallocator.alloc(layout) }
                } else {
                    allocate_core(layout, false)
                };
                assert!(!pointer.is_null());
                assert!(reusable.contains(&pointer));
                let link_bytes = std::mem::size_of::<usize>();
                // SAFETY: The fixture initializes the whole class to 19 and retains its live
                // peers; same-thread free-list operations overwrite only the first link word.
                let tail = unsafe { std::slice::from_raw_parts(pointer.wrapping_add(link_bytes), layout.size() - link_bytes) };
                assert_eq!(tail, &[19; 25]);
                for peer in held {
                    // SAFETY: Each held fixture peer is a distinct live allocation.
                    unsafe { crate::thread::deallocate(peer) };
                }
                // SAFETY: pointer remains the one live fixture allocation.
                unsafe { crate::thread::deallocate(pointer) };
            }
        });
    }

    #[test]
    fn gamma_round2_resize_publication_follows_release_and_both_events() {
        let mut old = 0u8;
        let mut new = 0u8;
        let old = std::ptr::from_mut(&mut old);
        let new = std::ptr::from_mut(&mut new);
        let layout = Layout::from_size_align(3, 1).unwrap();
        for retained in [false, true] {
            let events = std::cell::RefCell::new(Vec::new());
            let published = std::cell::Cell::new(false);
            let replacement = if retained { old } else { new };
            record_successful_resize(
                (old, layout),
                (replacement, 7),
                retained,
                |address, size, alignment, is_free| {
                    assert_eq!(alignment, 1);
                    if is_free {
                        assert_eq!((address, size), (old, 3));
                        events.borrow_mut().push("freed");
                    } else {
                        assert_eq!((address, size), (replacement, 7));
                        events.borrow_mut().push("allocated");
                    }
                },
                |address| {
                    assert_eq!(address, old);
                    events.borrow_mut().push("released");
                },
                || {
                    published.set(true);
                    events.borrow_mut().push("published");
                },
            );
            let mut expected_events = vec!["freed"];
            if !retained {
                expected_events.push("released");
            }
            expected_events.push("allocated");
            expected_events.push("published");
            assert_eq!(*events.borrow(), expected_events);
            assert!(published.get());
        }
    }

    #[test]
    fn gamma_round2_capture_failures_keep_stage_diagnostics_and_short_circuit() {
        let inventory_error = capture_with_inventory(|_, _, budget| {
            assert_eq!(budget, crate::observation::WALK_BUDGET);
            Err(seismograph::Error::new("inventory exhausted"))
        })
        .unwrap_err();
        assert_eq!(inventory_error.to_string(), "inventory exhausted");
        let size_error = encode_capture_with(
            || Err::<usize, ()>(()),
            |_| panic!("sizing failure must not allocate"),
            |_| panic!("sizing failure must not encode"),
        )
        .unwrap_err();
        assert_eq!(size_error.to_string(), "rallocator source sizing failed");
        let allocate_error = encode_capture_with(
            || Ok::<usize, ()>(7),
            |len| {
                assert_eq!(len, 7);
                Err(seismograph::Error::new("capture storage exhausted"))
            },
            |_| panic!("allocation failure must not encode"),
        )
        .unwrap_err();
        assert_eq!(allocate_error.to_string(), "capture storage exhausted");
        let encode_error = encode_capture_with(
            || Ok(7),
            seismograph::snapshot::SourceData::zeroed,
            |bytes| {
                assert_eq!(bytes, [0; 7]);
                Err(())
            },
        )
        .unwrap_err();
        assert_eq!(encode_error.to_string(), "rallocator source encoding failed");
        let mut success = encode_capture_with(
            || Ok::<usize, ()>(3),
            seismograph::snapshot::SourceData::zeroed,
            |bytes| {
                bytes.copy_from_slice(&[11, 23, 47]);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(success.as_mut_bytes(), [11, 23, 47]);
    }

    fn gamma_survivor_prime_destination(layout: Layout) -> (Vec<*mut u8>, Vec<*mut u8>, usize) {
        crate::thread::flush();
        let request = crate::classes::Request::new(layout).unwrap();
        let index = request.tag() - crate::classes::SMALL_TAG;
        let class = crate::classes::CLASSES[index];
        let mut held = Vec::new();
        loop {
            let pointer = allocate_core(layout, false);
            assert!(!pointer.is_null());
            // SAFETY: The internal allocator grants this complete Request class.
            unsafe { pointer.write_bytes(19, class.size) };
            held.push(pointer);
            assert!(held.len() <= 4096);
            let mut available = false;
            crate::thread::observe_current(|owner| {
                // SAFETY: This synchronous callback runs on the core's sole owner thread.
                let state = unsafe { &*owner.core_ptr() }.observe(&mut 0);
                available = state.classes[index].fast_nonempty || state.classes[index].available_slabs != 0;
            });
            if !available {
                break;
            }
        }
        let base = held[0].addr() & !(class.slab - 1);
        let mut reusable = Vec::new();
        held.retain(|&pointer| {
            if pointer.addr() & !(class.slab - 1) == base && reusable.len() < usize::from(class.waking) {
                reusable.push(pointer);
                // SAFETY: Each initialized destination allocation is retired only once.
                unsafe { crate::thread::deallocate(pointer) };
                false
            } else {
                true
            }
        });
        assert_eq!(reusable.len(), usize::from(class.waking));
        (reusable, held, class.size)
    }

    #[test]
    fn gamma_survivor_suppressed_recorded_resize_preserves_copy_and_release() {
        without_recording(|| {
            for (old_size, new_size, alignment, retained) in [
                (33, 47, 16, true),
                (33, 48, 16, true),
                (33, 80, 16, false),
                (80, 33, 16, false),
                (65536, 65537, 16, false),
                (8192, 8192, 16, true),
            ] {
                let layout = Layout::from_size_align(old_size, alignment).unwrap();
                let original = allocate_core(layout, false);
                assert!(!original.is_null());
                // SAFETY: The nonnull allocation covers every byte initialized here.
                unsafe { original.write_bytes(97, old_size) };
                let primed = (!retained && new_size <= 65_536).then(|| {
                    let new_layout = Layout::from_size_align(new_size, alignment).unwrap();
                    gamma_survivor_prime_destination(new_layout)
                });
                let released = std::cell::Cell::new(None);
                let release = |pointer| {
                    released.set(Some(pointer));
                    // SAFETY: The resize callback retires the live original once after copying.
                    unsafe { crate::thread::deallocate(pointer) };
                };
                // SAFETY: original is live for layout and every new layout is valid and nonzero.
                let replacement = unsafe { reallocate_recorded_with_release(original, layout, new_size, release) };
                assert!(!replacement.is_null());
                assert_eq!(replacement == original, retained);
                assert_eq!(released.get(), (!retained).then_some(original));
                // SAFETY: Successful resize preserves the initialized old/new intersection.
                let prefix = unsafe { std::slice::from_raw_parts(replacement, old_size.min(new_size)) };
                assert!(prefix.iter().all(|&byte| byte == 97));
                if let Some((reusable, held, class_size)) = primed {
                    assert!(
                        reusable.contains(&replacement),
                        "resize {old_size}->{new_size}: {replacement:p} not in reusable {reusable:?}"
                    );
                    let tail_start = replacement.wrapping_add(new_size);
                    // SAFETY: This same class-sized allocation was initialized before cache return;
                    // resize overwrites only its requested prefix, leaving the known initialized tail.
                    let tail = unsafe { std::slice::from_raw_parts(tail_start, class_size - new_size) };
                    assert!(tail.iter().all(|&byte| byte == 19));
                    for pointer in held {
                        // SAFETY: These distinct fixture peers remain live throughout the resize.
                        unsafe { crate::thread::deallocate(pointer) };
                    }
                }
                // SAFETY: replacement is the one live allocation retained or returned by resize.
                unsafe { crate::thread::deallocate(replacement) };
            }
        });
    }

    #[test]
    fn gamma_survivor_suppressed_recorded_allocation_zeroes_reused_bytes() {
        without_recording(|| {
            let layout = Layout::from_size_align(33, 16).unwrap();
            let (reusable, held, _) = gamma_survivor_prime_destination(layout);
            let zeroed = allocate_recorded(layout, true);
            assert!(!zeroed.is_null());
            assert!(reusable.contains(&zeroed));
            // SAFETY: The recorded zeroed allocation initializes all requested bytes.
            let bytes = unsafe { std::slice::from_raw_parts(zeroed, layout.size()) };
            assert_eq!(bytes, [0; 33]);
            for pointer in held {
                // SAFETY: Fixture peers are distinct live allocations, each retired once.
                unsafe { crate::thread::deallocate(pointer) };
            }
            // SAFETY: zeroed is a live allocation of layout, retired exactly once.
            unsafe { deallocation_recorded(zeroed, layout) };
        });
    }

    #[test]
    fn moved_realloc_records_free_before_cross_thread_address_reuse() {
        const NAME: &str = "recording::tests::moved_realloc_records_free_before_cross_thread_address_reuse";
        if std::env::var("RALLOCATOR_REALLOC_RECORDING_TEST").as_deref() != Ok("1") {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env("RALLOCATOR_REALLOC_RECORDING_TEST", "1")
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            return;
        }
        seismograph::recorder(seismograph::recorder::Configuration {
            allocations: seismograph::recorder::Policy::all(false),
            ..Default::default()
        });
        let layout = Layout::from_size_align(2 << 20, 16).unwrap();
        let left = allocate(layout, false);
        let original = allocate(layout, false);
        let right = allocate(layout, false);
        assert!(!left.is_null() && !original.is_null() && !right.is_null());
        let (request, requests) = std::sync::mpsc::channel();
        let (ready, responses) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            // Warm this owner's metadata before it can consume the returned span.
            let warm_layout = Layout::from_size_align(32, 16).unwrap();
            let warm = allocate(warm_layout, false);
            assert!(!warm.is_null());
            // SAFETY: The warm allocation is still live with its original layout.
            unsafe { deallocate(warm, warm_layout) };
            ready.send(0).unwrap();
            let expected = requests.recv().unwrap();
            let reused = allocate(layout, false);
            assert_eq!(reused.addr(), expected);
            ready.send(reused.addr()).unwrap();
            requests.recv().unwrap();
            // SAFETY: The worker owns this replacement for its entire lifetime.
            unsafe { deallocate(reused, layout) };
        });
        responses.recv().unwrap();
        let release = |pointer: *mut u8| {
            let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
            let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
            assert!(decoded.events.events.iter().any(|event| {
                event.kind == seismograph::recorder::event::EventKind::Deallocation
                    && event
                        .allocation()
                        .is_some_and(|allocation| allocation.address.get() == pointer.addr() as u64)
            }));
            // SAFETY: The callback receives the still-live old allocation once.
            unsafe { crate::thread::deallocate(pointer) };
            request.send(pointer.addr()).unwrap();
            responses.recv().unwrap();
        };
        // SAFETY: The original is live; the callback retires it once after its
        // free is recorded and leaves the reused allocation live for capture.
        let replacement = unsafe { reallocate_recorded_with_release(original, layout, 4 << 20, release) };
        assert!(!replacement.is_null());
        let snapshot = seismograph::snapshot(seismograph::snapshot::SnapshotOptions::default()).unwrap();
        let decoded = seismograph::snapshot::decode(snapshot.as_bytes()).unwrap();
        let callers = seismograph_rallocator::events::callers(&decoded.events);
        let matching = callers
            .events
            .iter()
            .filter(|event| event.address == original.addr() as u64)
            .collect::<Vec<_>>();
        assert_eq!(matching.len(), 3);
        let free = matching
            .iter()
            .find(|event| event.kind == seismograph_rallocator::callers::EventKind::Deallocated)
            .unwrap();
        assert!(free.allocation_recorded);
        assert_eq!(matching.iter().filter(|event| event.allocation_id == free.allocation_id).count(), 2);
        request.send(0).unwrap();
        worker.join().unwrap();
        // SAFETY: These three allocations remain live and are retired once.
        unsafe { deallocate(left, layout) };
        // SAFETY: The right guard allocation remains live with its original layout.
        unsafe { deallocate(right, layout) };
        // SAFETY: A successful moved realloc owns the new layout.
        unsafe { deallocate(replacement, Layout::from_size_align(4 << 20, 16).unwrap()) };
        seismograph::recorder(seismograph::recorder::Configuration::default());
    }
}
