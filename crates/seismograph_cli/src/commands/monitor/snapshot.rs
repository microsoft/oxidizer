// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::data::{AllocationSnapshot, CapturedSnapshot, MemorySnapshot, RuntimeSnapshot, deallocated_allocations};
use super::filter_index::FilterIndex;

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
    let mut allocator = None;
    let mut runtime_source = None;
    for source in std::mem::take(&mut decoded.sources) {
        if source.id == seismograph_rallocator::source::ID {
            progress(Phase::DecodeAllocator);
            let mut snapshot = seismograph_rallocator::decode(&source.data)
                .map_err(super::Error::MemorySnapshot)
                .map_err(|error| error.to_string())?;
            // The container owns the authoritative runtime events, not an optional legacy copy.
            snapshot.runtime_events = None;
            allocator = Some(snapshot);
        } else if source.id == seismograph_runtime::snapshot::source::ID {
            progress(Phase::DecodeRuntime);
            runtime_source =
                Some(seismograph_runtime::snapshot::decode(&source.data).map_err(|error| format!("invalid runtime snapshot: {error}"))?);
        }
    }

    progress(Phase::AllocationIndex);
    let deallocated = allocator.as_ref().map(deallocated_allocations).unwrap_or_default();
    progress(Phase::Heaps);
    let memory = allocator
        .as_ref()
        .map(|snapshot| MemorySnapshot::from_snapshot_with_deallocated(snapshot, &deallocated));
    progress(Phase::Allocations);
    let allocations = allocator
        .as_ref()
        .map(|snapshot| AllocationSnapshot::from_snapshot_with_deallocated(snapshot, &deallocated));
    let heap_error = allocator
        .is_none()
        .then(|| format!("heap data unavailable: {}", super::Error::MissingMemorySource));
    progress(Phase::Symbols);
    let mut addresses = allocator
        .iter()
        .flat_map(|allocator| allocator.addresses.iter().cloned())
        .map(|lookup| (lookup.address, lookup))
        .collect::<std::collections::BTreeMap<_, _>>();
    if let Some(source) = &mut runtime_source {
        for lookup in std::mem::take(&mut source.addresses) {
            addresses.insert(
                lookup.address,
                seismograph_rallocator::callers::AddressLookup::from_fields(seismograph_rallocator::callers::AddressLookupFields {
                    address: lookup.address,
                    symbol: lookup.symbol,
                    filename: lookup.filename,
                    line: lookup.line,
                    column: lookup.column,
                }),
            );
        }
    }
    let addresses = addresses.into_values().collect::<Vec<_>>();
    let runtime = RuntimeSnapshot::from_events_with_progress(&decoded, &addresses, runtime_source.as_ref(), progress);
    progress(Phase::IndexStacks);
    let filter_index = std::sync::Arc::new(FilterIndex::new(decoded, allocator, runtime_source, addresses, deallocated));
    let filter_summary = filter_index.unfiltered_summary();
    progress(Phase::Ready);
    Ok(Box::new(CapturedSnapshot {
        memory,
        allocations,
        heap_error,
        primitives: runtime.primitives,
        runtime: runtime.runtime,
        io: runtime.io,
        cache: runtime.cache,
        threads: runtime.threads,
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
    use seismograph::recorder::thread::{ThreadId, ThreadLog};
    use seismograph::snapshot::{DecodedSnapshot, SourceSnapshot};
    use seismograph_rallocator::callers::{AddressLookup, Callers};

    use super::{prepare, release_chunks, release_stacks};

    fn allocator_source(snapshot: &seismograph_rallocator::snapshot::Snapshot) -> SourceSnapshot {
        let mut data = vec![0; seismograph_rallocator::encoded_len(snapshot).unwrap()];
        seismograph_rallocator::encode(snapshot, &mut data).unwrap();
        SourceSnapshot {
            id: seismograph_rallocator::source::ID,
            name: "allocator".into(),
            schema_version: 1,
            data,
        }
    }

    fn runtime_source() -> SourceSnapshot {
        // Runtime wire version 1, schema 3: no runtimes and one symbol lookup.
        let symbol = b"application::runtime";
        let mut data = b"SEISRUNT".to_vec();
        data.extend_from_slice(&1_u16.to_le_bytes());
        data.extend_from_slice(&3_u16.to_le_bytes());
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
            schema_version: 3,
            data,
        }
    }

    #[test]
    fn preparation_uses_container_events_and_runtime_symbols_over_legacy_copies() {
        let mut allocator = seismograph_rallocator::snapshot::Snapshot::new(seismograph_rallocator::snapshot::Version::new(0, 1, 0));
        allocator.callers = Some(Callers::default());
        allocator.runtime_events = Some(Events {
            clock: seismograph::recorder::event::EventClock::ProcessMonotonic,
            total_events: 99,
            threads: vec![ThreadLog {
                thread_id: ThreadId::new(2),
                name: "legacy".into(),
                total_events: 99,
                lost_events: 0,
            }],
            ..Default::default()
        });
        let mut lookup = AddressLookup::default();
        lookup.address = 16;
        lookup.symbol = Some("application::allocator".into());
        allocator.addresses.push(lookup.clone());
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
                true,
                Some(format!("heap data unavailable: {}", super::super::Error::MissingMemorySource)),
            ),
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
