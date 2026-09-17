// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::alloc::{GlobalAlloc, Layout};

use allocation_hints::heaps::{Heap, bump, general};
use allocation_hints::with_hint;

use crate::common::{Block, check_disjoint};

pub(crate) const MAX_OPERATIONS: usize = if cfg!(miri) { 12 } else { 96 };
pub(crate) const INPUT_BYTES: usize = 1 + 6 * MAX_OPERATIONS;
const MAX_LIVE: usize = if cfg!(miri) { 4 } else { 16 };

pub(crate) struct Recording;

impl Recording {
    pub(crate) fn new(enabled: bool) -> Self {
        seismograph::recorder(seismograph::recorder::Configuration {
            allocations: seismograph::recorder::RecordingPolicy {
                enabled,
                capture_backtraces: false,
                ..Default::default()
            },
            ..Default::default()
        });
        Self
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        seismograph::recorder(seismograph::recorder::Configuration::default());
    }
}

/// Replays only valid allocator operations, with a fixed live set and work bound.
///
/// The first byte controls recording. Each subsequent six-byte instruction
/// selects an operation, live index, size, size adjustment, alignment, and heap.
pub(crate) fn run<A: GlobalAlloc>(allocator: &A, input: &[u8]) {
    let recording = input.first().is_some_and(|byte| byte & 1 != 0);
    let trace = std::env::var_os("RALLOCATOR_STRESS_TRACE").is_some();
    let _recording = Recording::new(recording);
    let mut heaps = [new_general(), new_bump()];
    let mut live = Vec::with_capacity(MAX_LIVE);
    for (step, instruction) in input.get(1..).unwrap_or_default().chunks_exact(6).take(MAX_OPERATIONS).enumerate() {
        check_disjoint(&live);
        let operation = instruction[0] % 8;
        let index = usize::from(instruction[1]) % live.len().max(1);
        let size = allocation_size(instruction[0] & 0x08 != 0, instruction[2], instruction[3]);
        let alignment = 1 << (instruction[4] % if cfg!(miri) { 8 } else { 18 });
        let layout = Layout::from_size_align(size, alignment).unwrap();
        if trace {
            // Unbuffered so a step that never returns is still attributable.
            eprintln!(
                "step={step} operation={operation} index={index} size={size} alignment={alignment} heap={} live={}",
                instruction[5] % 4,
                live.len()
            );
        }
        let tag = (u64::try_from(step).unwrap() + 1) << 32 | u64::from(u16::from_le_bytes([instruction[2], instruction[3]]));
        if live.is_empty() || operation <= 1 {
            if live.len() == MAX_LIVE {
                drop(live.swap_remove(index));
            }
            let block = in_heap(&heaps, instruction[5], || Block::new(allocator, layout, tag, operation == 1));
            live.push(block);
        } else {
            match operation {
                2 => in_heap(&heaps, instruction[5], || live[index].reallocate(size)),
                3 => drop(live.swap_remove(index)),
                4 => live[index].paint(tag),
                5 => live[index].check(),
                6 => {
                    // Live allocations can escape a logical heap and survive
                    // eviction of its cached native attachment.
                    let heap = usize::from(instruction[5]) % heaps.len();
                    heaps[heap] = if heap == 0 { new_general() } else { new_bump() };
                }
                _ => {
                    while live.len() > 1 {
                        drop(live.swap_remove(live.len() / 2));
                    }
                }
            }
        }
        check_disjoint(&live);
    }
    check_disjoint(&live);
    drop(live);
    drop(heaps);
    // A subsequent allocator operation observes that the last hint ended.
    drop(Block::new(allocator, Layout::new::<u64>(), u64::MAX, false));
}

/// Chooses a request size, either from the boundary table or freely.
///
/// `arbitrary` comes from a spare operation bit rather than the size bytes, so
/// free sizes span their whole range instead of only those whose low byte has
/// its high bit set.
fn allocation_size(arbitrary: bool, selector: u8, adjustment: u8) -> usize {
    if cfg!(miri) {
        const SIZES: &[usize] = &[1, 15, 16, 17, 63, 64, 65, 255, 256, 257];
        SIZES[usize::from(selector) % SIZES.len()]
    } else if arbitrary {
        usize::from(u16::from_le_bytes([selector, adjustment])) + 1
    } else {
        // Sizes straddling allocator boundaries, including the bump chunk
        // segment capacity: a defect there occupied a window only a few bytes
        // wide, which uniform sampling is very unlikely to reach.
        const SIZES: &[usize] = &[
            1, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255, 256, 257, 511, 512, 513, 1_023, 1_024, 1_025, 4_095, 4_096, 4_097,
            4_383, 4_384, 4_385, 8_191, 8_192, 8_193, 16_383, 16_384, 16_385, 32_687, 32_688, 32_689, 32_703, 32_704, 32_705, 32_712,
            32_713, 32_714, 32_767, 32_768, 32_769, 65_535, 65_536, 65_537, 131_071, 131_072, 131_073,
        ];
        SIZES[usize::from(selector) % SIZES.len()]
    }
}

fn in_heap<T>(heaps: &[Heap; 2], selection: u8, action: impl FnOnce() -> T) -> T {
    match selection % 4 {
        0 => action(),
        1 => with_hint(&heaps[0], action),
        2 => with_hint(&heaps[1], action),
        _ => with_hint(&heaps[0], || with_hint(&heaps[1], action)),
    }
}

fn new_general() -> Heap {
    Heap::general(
        general::Options::new()
            .with_locality_segment_bytes(64 * 1024)
            .with_medium_cache_max_bytes(0),
    )
}

fn new_bump() -> Heap {
    Heap::bump(bump::Options::new().with_retained_chunks(1))
}
