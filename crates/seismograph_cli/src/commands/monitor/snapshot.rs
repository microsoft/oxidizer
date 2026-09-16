// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::data::{AllocationSnapshot, CapturedSnapshot, MemorySnapshot, RuntimeSnapshot, deallocated_allocations};

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
    ReleaseEvents,
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
    // Move symbol strings rather than duplicating the symbol tables.
    progress(Phase::Symbols);
    drop(deallocated);
    if let Some(callers) = allocator.as_mut().and_then(|snapshot| snapshot.callers.as_mut()) {
        release_stacks(&mut callers.events, |event| drop(std::mem::take(&mut event.call_stack)));
    }
    let mut addresses = allocator
        .take()
        .into_iter()
        .flat_map(|allocator| allocator.addresses)
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
    progress(Phase::ReleaseEvents);
    release_stacks(&mut decoded.events.events, |event| drop(std::mem::take(&mut event.call_stack)));
    drop(decoded);
    drop(runtime_source);
    drop(addresses);
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
    }))
}

fn release_stacks<T: Send>(events: &mut [T], release: impl Fn(&mut T) + Sync) {
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
        for chunk in events.chunks_mut(chunk_size) {
            if std::thread::Builder::new()
                .name("seismograph-release".into())
                .spawn_scoped(scope, move || {
                    for event in chunk {
                        release(event);
                    }
                })
                .is_err()
            {
                // Any unprocessed stacks remain owned by their events and drop normally.
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::release_stacks;

    #[test]
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
