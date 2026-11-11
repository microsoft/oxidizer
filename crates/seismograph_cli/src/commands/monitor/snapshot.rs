// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::data::{AllocationSnapshot, CapturedSnapshot, MemorySnapshot, RuntimeSnapshot, deallocated_allocations};
use super::filter_index::FilterIndex;

pub(super) fn heap_error(allocator: Option<&crate::allocator_view::Snapshot>) -> Option<String> {
    allocator
        .is_none_or(|snapshot| snapshot.native.is_none() && snapshot.mimalloc.is_none())
        .then(|| "Native allocator source unavailable; container allocation events remain available.".into())
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Phase {
    Read,
    DecodeContainer,
    ReleaseInput,
    DecodeAllocator,
    DecodeRuntime,
    AllocationIndex,
    Heaps,
    Allocations,
    Symbols,
    Primitives,
    Runtime,
    Io,
    Cache,
    Threads,
    IndexStacks,
    Ready,
}

/// Builds the shared TUI model, consuming source payloads before summarizing events.
pub(super) fn prepare(decoded: seismograph::snapshot::DecodedSnapshot) -> Result<Box<CapturedSnapshot>, String> {
    prepare_with_progress(decoded, &mut |_| {})
}

pub(super) fn prepare_with_progress(
    mut decoded: seismograph::snapshot::DecodedSnapshot,
    progress: &mut impl FnMut(Phase),
) -> Result<Box<CapturedSnapshot>, String> {
    if decoded
        .sources
        .iter()
        .any(|source| source.id == seismograph_rallocator::source::ID || source.id == seismograph_rallocator::source::MIMALLOC_ID)
    {
        progress(Phase::DecodeAllocator);
    }
    let mut allocator = Some(crate::allocator_sources::decode(&decoded.sources).map_err(|error| match error {
        crate::snapshot_error::Error::DecodeAllocator(error) => super::Error::MemorySnapshot(error).to_string(),
        error => error.to_string(),
    })?);
    let mut runtime_source = None;
    for source in std::mem::take(&mut decoded.sources) {
        if source.id == seismograph_runtime::snapshot::source::ID {
            progress(Phase::DecodeRuntime);
            if source.schema_version != seismograph_runtime::snapshot::source::SCHEMA_VERSION {
                return Err(crate::snapshot_error::Error::UnsupportedRuntimeSchema(source.schema_version).to_string());
            }
            runtime_source =
                Some(seismograph_runtime::snapshot::decode(&source.data).map_err(|error| format!("invalid runtime snapshot: {error}"))?);
        }
    }
    // Application events do not depend on the presence of an allocator source.
    let view = allocator.get_or_insert_with(|| crate::allocator_view::Snapshot::event_only(crate::allocator_view::Version::new(0, 1, 0)));
    let mut callers = seismograph_rallocator::events::callers(&decoded.events);
    callers.session_id = view.session_id();
    let identities = crate::allocation_identities::from_callers(&callers);
    view.callers = Some(callers);

    progress(Phase::AllocationIndex);
    let deallocated = allocator.as_ref().map(deallocated_allocations).unwrap_or_default();
    progress(Phase::Heaps);
    let memory = allocator
        .as_ref()
        .filter(|snapshot| snapshot.allocator_state_available)
        .map(|snapshot| MemorySnapshot::from_snapshot_with_deallocated(snapshot, &deallocated));
    let heap_error = heap_error(allocator.as_ref());
    progress(Phase::Symbols);
    let addresses = crate::snapshot_symbols::merge(
        allocator.as_mut().map_or_else(Vec::new, |view| std::mem::take(&mut view.addresses)),
        runtime_source
            .as_mut()
            .map_or_else(Vec::new, |source| std::mem::take(&mut source.addresses)),
        std::mem::take(&mut decoded.symbols),
    );
    if let Some(view) = &mut allocator {
        view.addresses.clone_from(&addresses);
    }
    progress(Phase::Allocations);
    let allocations = allocator
        .as_ref()
        .map(|snapshot| AllocationSnapshot::from_snapshot_with_deallocated(snapshot, &deallocated));
    let runtime = RuntimeSnapshot::from_events_with_progress(&decoded, &addresses, runtime_source.as_ref(), identities.as_ref(), progress);
    progress(Phase::IndexStacks);
    let native = allocator.as_ref().and_then(|snapshot| snapshot.native.clone());
    let mimalloc = allocator.as_ref().and_then(|snapshot| snapshot.mimalloc.clone());
    let filter_index = std::sync::Arc::new(FilterIndex::with_task_events(
        decoded,
        allocator,
        runtime_source,
        addresses,
        deallocated,
        runtime.task_events.clone(),
        identities,
    ));
    let filter_summary = filter_index.unfiltered_summary();
    progress(Phase::Ready);
    Ok(Box::new(CapturedSnapshot {
        native,
        mimalloc,
        memory,
        allocations,
        heap_error,
        primitives: runtime.primitives,
        runtime: runtime.runtime,
        io: runtime.io,
        cache: runtime.cache,
        threads: runtime.threads,
        task_events: runtime.task_events,
        captured_at: None,
        captured_instant: None,
        filter_index: Some(filter_index),
        filter_summary,
    }))
}

pub(super) fn release_stacks<T: Send>(events: &mut [T], release: impl Fn(&mut T) + Sync) {
    // Large captures own tens of millions of independent stack allocations. Bound
    // reclamation parallelism so freeing them does not monopolize the whole host.
    const MINIMUM_EVENTS: usize = 1_000_000;
    const MAXIMUM_WORKERS: usize = 8;
    if events.len() < MINIMUM_EVENTS {
        return;
    }
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .min(MAXIMUM_WORKERS);
    let chunk_size = events.len().div_ceil(workers);
    std::thread::scope(|scope| {
        let release = &release;
        release_chunks(events.chunks_mut(chunk_size), |chunk| {
            std::thread::Builder::new()
                .name("seismograph-release".into())
                .spawn_scoped(scope, move || {
                    for event in chunk {
                        release(event);
                    }
                })
                .map(|_| ())
        });
    });
}

fn release_chunks<T>(chunks: impl Iterator<Item = T>, mut release: impl FnMut(T) -> std::io::Result<()>) {
    for chunk in chunks {
        if release(chunk).is_err() {
            // Any unprocessed stacks remain owned by their events and drop normally.
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use seismograph::recorder::event::{Address, Event, EventKind, EventPayload, EventSequence, EventTimestamp, Events, ObjectId};
    use seismograph::recorder::thread::ThreadId;
    use seismograph::snapshot::{DecodedSnapshot, SourceSnapshot};
    use seismograph_rallocator::callers::AddressLookup;

    use super::{prepare, release_chunks, release_stacks};

    #[test]
    fn previous_runtime_envelopes_are_rejected_before_payload_decoding() {
        for version in 0..seismograph_runtime::snapshot::source::SCHEMA_VERSION {
            let mut source = runtime_source();
            source.schema_version = version;
            source.data.clear();
            let error = prepare(DecodedSnapshot {
                sources: vec![source],
                ..DecodedSnapshot::default()
            })
            .err()
            .unwrap();
            assert_eq!(error, format!("unsupported runtime source schema {version}"));
        }
    }

    #[test]
    fn filtering_a_resize_preserves_the_original_thread_lifetime_relationship() {
        use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind, Reallocation};

        use super::super::data::ThreadOperationKind;
        use super::super::filter::{FilterSpec, RuntimeStackMode};

        let allocation = Allocation {
            allocation_id: AllocationId::new(0x8000),
            event_thread_id: EventThreadId::new(1001),
            heap_id: HeapId::new(0),
            heap_kind: HeapKind::General,
            freed_after_heap_release: false,
            address: Address::new(0x8000),
            size: 64,
            alignment: 16,
        };
        let raw = [
            (EventKind::Allocation, EventPayload::Allocation(allocation), 0x100),
            (
                EventKind::Reallocation,
                EventPayload::Reallocation(Reallocation {
                    allocation_id: allocation.allocation_id,
                    heap_id: allocation.heap_id,
                    heap_kind: allocation.heap_kind,
                    old_address: allocation.address,
                    new_address: Address::new(0x9000),
                    old_size: 64,
                    new_size: 4096,
                    alignment: 16,
                }),
                0x200,
            ),
            (
                EventKind::Deallocation,
                EventPayload::Allocation(Allocation {
                    allocation_id: AllocationId::new(0x9000),
                    event_thread_id: EventThreadId::new(1003),
                    address: Address::new(0x9000),
                    size: 4096,
                    ..allocation
                }),
                0x100,
            ),
        ];
        let decoded = DecodedSnapshot {
            events: Events {
                events: raw
                    .into_iter()
                    .enumerate()
                    .map(|(index, (kind, payload, frame))| Event {
                        thread_id: ThreadId::new(index as u64 + 1),
                        sequence: EventSequence::new(1),
                        timestamp: EventTimestamp::from_ticks(index as u64 + 1),
                        kind,
                        payload,
                        call_stack: vec![Address::new(frame)],
                    })
                    .collect(),
                ..Events::default()
            },
            symbols: [(0x100, "app::work"), (0x200, "noise::resize")]
                .map(|(address, name)| seismograph::snapshot::Symbol {
                    address: Address::new(address),
                    name: Some(name.into()),
                    filename: None,
                    line: None,
                    column: None,
                })
                .into(),
            ..DecodedSnapshot::default()
        };

        let original = prepare(decoded).unwrap();
        let participants = |snapshot: &super::super::data::CapturedSnapshot| {
            snapshot
                .threads
                .threads
                .iter()
                .find(|thread| thread.thread_id == 1)
                .unwrap()
                .operations
                .iter()
                .find(|operation| operation.kind == ThreadOperationKind::Allocation)
                .unwrap()
                .participants
                .iter()
                .map(|participant| participant.thread_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(participants(&original), [2, 3]);
        let filtered = original
            .filter_index
            .as_ref()
            .unwrap()
            .render(&FilterSpec::parse("crate:app", "", false, RuntimeStackMode::Event).unwrap());

        assert_eq!(participants(&filtered), [3]);
        let allocations = filtered.allocations.as_ref().unwrap();
        assert_eq!(allocations.records.len(), 2);
        assert_eq!(
            (allocations.hotspots[0].allocations, allocations.hotspots[0].live_allocations),
            (1, 0)
        );
    }

    fn allocator_source(snapshot: &seismograph_rallocator::native::Snapshot) -> SourceSnapshot {
        let mut data = vec![0; seismograph_rallocator::encoded_len(snapshot).unwrap()];
        seismograph_rallocator::encode(snapshot, &mut data).unwrap();
        SourceSnapshot {
            id: seismograph_rallocator::source::ID,
            name: "allocator".into(),
            schema_version: seismograph_rallocator::source::SCHEMA_VERSION,
            data,
        }
    }

    fn runtime_source() -> SourceSnapshot {
        // Current runtime schema: no runtimes and one symbol lookup.
        let symbol = b"application::runtime";
        let mut data = b"SEISRUNT".to_vec();
        data.extend_from_slice(&1_u16.to_le_bytes());
        data.extend_from_slice(&seismograph_runtime::snapshot::source::SCHEMA_VERSION.to_le_bytes());
        data.extend_from_slice(&0_u32.to_le_bytes());
        data.extend_from_slice(&1_u32.to_le_bytes());
        data.extend_from_slice(&16_u64.to_le_bytes());
        data.extend_from_slice(&u32::try_from(symbol.len()).unwrap().to_le_bytes());
        data.extend_from_slice(&0_u32.to_le_bytes());
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        data.extend_from_slice(&u32::MAX.to_le_bytes());
        data.extend_from_slice(symbol);
        SourceSnapshot {
            id: seismograph_runtime::snapshot::source::ID,
            name: "runtime".into(),
            schema_version: seismograph_runtime::snapshot::source::SCHEMA_VERSION,
            data,
        }
    }

    #[test]
    fn preparation_uses_container_events_and_runtime_source_symbols() {
        let allocator = seismograph_rallocator::native::Snapshot::default();
        let mut lookup = AddressLookup::default();
        lookup.address = 16;
        lookup.symbol = Some("application::allocator".into());
        let decoded = DecodedSnapshot {
            events: Events {
                total_events: 1,
                events: vec![Event {
                    thread_id: ThreadId::new(1),
                    sequence: EventSequence::new(1),
                    timestamp: EventTimestamp::from_ticks(1),
                    kind: EventKind::ArcClone,
                    payload: EventPayload::Object(ObjectId::new(7)),
                    call_stack: vec![Address::new(16)],
                }],
                ..Default::default()
            },
            sources: vec![allocator_source(&allocator), runtime_source()],
            ..Default::default()
        };
        lookup.symbol = Some("application::runtime".into());
        let expected = super::RuntimeSnapshot::from_events(&decoded, &[lookup], None);
        let snapshot = prepare(decoded).unwrap();
        assert_eq!(
            (
                snapshot.primitives,
                snapshot.threads,
                snapshot.heap_error,
                snapshot.captured_at,
                snapshot.captured_instant
            ),
            (expected.primitives, expected.threads, None, None, None),
        );
    }

    #[test]
    fn native_source_and_unknown_inventory_survive_event_filtering() {
        let allocator = seismograph_rallocator::native::Snapshot::default();
        let snapshot = prepare(DecodedSnapshot {
            sources: vec![allocator_source(&allocator)],
            ..Default::default()
        })
        .unwrap();
        assert!(snapshot.memory.is_none());
        assert!(snapshot.allocations.is_some());
        assert!(snapshot.heap_error.is_none());
        assert_eq!(snapshot.native.as_deref(), Some(&allocator));
        let filtered = snapshot
            .filter_index
            .as_ref()
            .unwrap()
            .render(&super::super::filter::FilterSpec::default());
        assert!(filtered.memory.is_none());
        assert!(filtered.allocations.is_some());
        assert_eq!(filtered.heap_error, snapshot.heap_error);
        assert_eq!(filtered.native, snapshot.native);
    }

    #[test]
    fn runtime_only_source_keeps_symbols_without_inventing_heap_data() {
        let snapshot = prepare(DecodedSnapshot {
            sources: vec![runtime_source()],
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            (snapshot.memory.is_none(), snapshot.allocations.is_none(), snapshot.heap_error),
            (
                true,
                false,
                Some("Native allocator source unavailable; container allocation events remain available.".into()),
            ),
        );
    }

    fn allocation_capture() -> DecodedSnapshot {
        use seismograph::recorder::alloc::{Allocation, AllocationId, EventThreadId, HeapId, HeapKind};
        let operations = [
            (EventKind::Allocation, 1, 0x100, 16),
            (EventKind::Deallocation, 2, 0x100, 32),
            (EventKind::Allocation, 2, 0x100, 16),
            (EventKind::Deallocation, 1, 0x100, 32),
            (EventKind::Deallocation, 2, 0x200, 32),
            (EventKind::Allocation, 1, 0x100, 16),
        ];
        DecodedSnapshot {
            events: Events {
                total_events: 6,
                threads: vec![
                    seismograph::recorder::thread::ThreadLog {
                        thread_id: ThreadId::new(1),
                        name: "owner".into(),
                        total_events: 3,
                        lost_events: 0,
                    },
                    seismograph::recorder::thread::ThreadLog {
                        thread_id: ThreadId::new(2),
                        name: "remote".into(),
                        total_events: 3,
                        lost_events: 0,
                    },
                ],
                events: operations
                    .into_iter()
                    .enumerate()
                    .map(|(index, (kind, thread, address, stack))| Event {
                        thread_id: ThreadId::new(thread),
                        sequence: EventSequence::new(index as u64 + 1),
                        timestamp: EventTimestamp::from_ticks(index as u64 + 1),
                        kind,
                        payload: EventPayload::Allocation(Allocation {
                            allocation_id: AllocationId::new(address),
                            event_thread_id: EventThreadId::new(0),
                            heap_id: HeapId::new(0),
                            heap_kind: HeapKind::General,
                            freed_after_heap_release: false,
                            address: Address::new(address),
                            size: 32,
                            alignment: 8,
                        }),
                        call_stack: vec![Address::new(stack)],
                    })
                    .collect(),
                ..Events::default()
            },
            sources: vec![runtime_source()],
            ..DecodedSnapshot::default()
        }
    }

    #[test]
    fn allocation_records_keep_actors_stacks_orphans_and_repeated_addresses_without_a_native_source() {
        let capture = prepare(allocation_capture()).unwrap();
        let allocations = capture.allocations.as_ref().unwrap();
        assert_eq!(allocations.records.len(), 6);
        assert!(allocations.records[0].label().contains("owner #1"));
        assert!(allocations.records[1].label().contains("remote #2"));
        assert!(allocations.records[0].label().contains("view #1"));
        assert!(allocations.records[1].label().contains("view #1"));
        assert!(allocations.records[2].label().contains("view #3"));
        assert!(allocations.records[3].label().contains("view #3"));
        assert!(allocations.records[4].label().contains("view #5"));
        assert!(allocations.records[4].label().contains("orphan free"));
        assert!(allocations.records[5].label().contains("view #6"));
        assert_eq!(
            allocations.records[0].stack(super::super::data::AllocationStackFilter::All),
            ["application::runtime"]
        );
        assert!(capture.native.is_none());
    }

    #[test]
    fn common_symbols_render_and_filter_allocations_without_a_source() {
        use super::super::data::AllocationStackFilter;
        use super::super::filter::{FilterSpec, RuntimeStackMode};

        let mut decoded = allocation_capture();
        decoded.sources.clear();
        decoded.symbols = vec![
            seismograph::snapshot::Symbol {
                address: Address::new(16),
                name: Some("application::allocate".into()),
                filename: Some("src\\allocate.rs".into()),
                line: Some(9),
                column: None,
            },
            seismograph::snapshot::Symbol {
                address: Address::new(32),
                name: Some("application::release".into()),
                filename: None,
                line: None,
                column: None,
            },
        ];

        let capture = prepare(decoded).unwrap();
        let filtered = capture
            .filter_index
            .as_ref()
            .unwrap()
            .render(&FilterSpec::parse("function:application::allocate", "", false, RuntimeStackMode::Event).unwrap());

        assert_eq!(
            capture.allocations.as_ref().unwrap().records[0].stack(AllocationStackFilter::All),
            ["application::allocate (allocate.rs:9)"]
        );
        let filtered = filtered.allocations.as_ref().unwrap();
        assert_eq!(filtered.records.len(), 3);
        assert!(
            filtered
                .records
                .iter()
                .all(|record| { record.stack(AllocationStackFilter::All) == ["application::allocate (allocate.rs:9)"] })
        );
        assert!(capture.native.is_none() && capture.mimalloc.is_none());
    }

    #[test]
    fn unresolved_common_symbols_preserve_distinct_frame_addresses() {
        use super::super::data::AllocationStackFilter;

        let mut decoded = allocation_capture();
        decoded.sources.clear();
        let unresolved = |address| seismograph::snapshot::Symbol {
            address: Address::new(address),
            name: None,
            filename: None,
            line: None,
            column: None,
        };
        decoded.symbols = vec![unresolved(16), unresolved(32)];

        let capture = prepare(decoded).unwrap();
        let records = &capture.allocations.as_ref().unwrap().records;

        assert_eq!(
            (
                records[0].stack(AllocationStackFilter::All),
                records[1].stack(AllocationStackFilter::All),
            ),
            (
                ["0x0000000000000010".to_owned()].as_slice(),
                ["0x0000000000000020".to_owned()].as_slice(),
            )
        );
    }

    #[test]
    fn native_coverage_and_original_event_correlations_survive_stack_filters() {
        use super::super::filter::{FilterSpec, RuntimeStackMode};
        let native = crate::native_view::fixture::snapshot();
        let mut decoded = allocation_capture();
        decoded.sources.push(allocator_source(&native));
        let capture = prepare(decoded).unwrap();
        let index = capture.filter_index.as_ref().unwrap();
        let allocations = index.render(&FilterSpec::parse("crate:application", "", false, RuntimeStackMode::Event).unwrap());
        let records = &allocations.allocations.as_ref().unwrap().records;
        assert_eq!(records.len(), 3);
        assert!(records[0].label().contains("view #1") && records[0].label().contains("matched"));
        assert!(records[1].label().contains("view #3") && records[1].label().contains("matched"));
        assert!(records[2].label().contains("view #6") && records[2].label().contains("unmatched"));
        assert_eq!(allocations.native.as_deref(), Some(&native));
        let frees = index.render(&FilterSpec::parse("", "crate:application", true, RuntimeStackMode::Event).unwrap());
        let records = &frees.allocations.as_ref().unwrap().records;
        assert_eq!(records.len(), 3);
        assert!(!records[0].label().contains("orphan"));
        assert!(!records[1].label().contains("orphan"));
        assert!(records[2].label().contains("orphan free"));
        assert_eq!(frees.native.as_deref(), Some(&native));
    }

    #[test]
    fn duplicate_or_malformed_native_source_is_not_silently_replaced() {
        let native = seismograph_rallocator::native::Snapshot::default();
        let mut unsupported = allocator_source(&native);
        unsupported.schema_version = 2;
        assert_eq!(
            prepare(DecodedSnapshot {
                sources: vec![unsupported],
                ..Default::default()
            })
            .err()
            .unwrap(),
            "unsupported allocator source schema 2"
        );
        assert!(
            prepare(DecodedSnapshot {
                sources: vec![allocator_source(&native), allocator_source(&native)],
                ..Default::default()
            })
            .is_err()
        );
        let mut source = allocator_source(&native);
        source.data.pop();
        assert!(
            prepare(DecodedSnapshot {
                sources: vec![source],
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn small_event_sets_keep_stacks_for_normal_drop() {
        let mut stacks = vec![vec![1_u64, 2], vec![3]];
        release_stacks(&mut stacks, Vec::clear);
        assert_eq!(stacks, [vec![1, 2], vec![3]]);
    }

    #[test]
    #[cfg_attr(miri, ignore = "million-event parallel release stress test requires native execution")]
    fn event_sets_at_the_parallel_release_threshold_are_released() {
        let mut events = vec![1_u8; 1_000_000];
        release_stacks(&mut events, |event| *event = 0);
        assert_eq!(events, vec![0; 1_000_000]);
    }

    #[test]
    #[cfg_attr(miri, ignore = "million-event parallel release stress test requires native execution")]
    fn large_event_sets_release_every_stack_before_returning() {
        let mut events = vec![1_u8; 1_000_003];
        release_stacks(&mut events, |event| *event = 0);
        assert_eq!(events, vec![0; 1_000_003]);
    }

    #[test]
    fn chunk_release_stops_on_spawn_failure_and_preserves_remaining_events() {
        let mut events = [1, 2, 3, 4];
        release_chunks(events.iter_mut(), |event| {
            if *event == 2 {
                return Err(std::io::ErrorKind::WouldBlock.into());
            }
            *event = 0;
            Ok(())
        });
        assert_eq!(events, [0, 2, 3, 4]);
    }

    #[test]
    #[cfg_attr(coverage_nightly, coverage(off))] // Manual profiling, not an automated test.
    #[ignore = "allocation-reclamation timing probe; run serially on the validation host"]
    fn stack_release_profile() {
        const COUNT: usize = 1_000_000;
        for parallel in [false, true] {
            let mut stacks = (0..COUNT).map(|_| vec![1_u64; 24]).collect::<Vec<_>>();
            let started = std::time::Instant::now();
            if parallel {
                release_stacks(&mut stacks, |stack| drop(std::mem::take(stack)));
                assert!(stacks.iter().all(Vec::is_empty));
            }
            drop(stacks);
            println!("parallel={parallel}: {:.3}s", started.elapsed().as_secs_f64());
        }
    }
}
