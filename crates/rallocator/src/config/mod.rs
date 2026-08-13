// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Compile-time allocator configurations.

use crate::tunables::{Standard as StandardTunables, Tunables};

/// Compile-time allocator configuration.
pub trait Config {
    type Tunables: Tunables;

    /// Whether aggregate allocation statistics are recorded.
    const TRACK_AGGREGATES: bool;

    /// Whether caller tracking can be enabled at runtime.
    const TRACK_CALLERS: bool;

    /// Number of caller events retained per participating thread.
    ///
    /// Enabling caller tracking lazily allocates a fixed-capacity event ring and
    /// stack-interning table for each thread that records an event. A completed
    /// session remains registered until the next session starts. A thread keeps
    /// its current session state until it exits or observes a later session.
    const CALLER_EVENT_CAPACITY: usize;

    /// Maximum allocation stack depth retained in caller events.
    const CALLER_ALLOCATION_STACK_FRAMES: usize;

    /// Maximum deallocation stack depth retained in caller events.
    const CALLER_DEALLOCATION_STACK_FRAMES: usize;

    /// Whether caller events identify the thread performing each operation.
    const CALLER_TRACK_THREADS: bool;

    /// Whether caller events retain heap identity and post-release free state.
    ///
    /// Heap kind is always recorded.
    const CALLER_TRACK_HEAP_LIFETIMES: bool;
}

/// Standard allocator configuration without tracking.
pub struct Standard;

impl Config for Standard {
    type Tunables = StandardTunables;

    const TRACK_AGGREGATES: bool = false;
    const TRACK_CALLERS: bool = false;
    const CALLER_EVENT_CAPACITY: usize = 128 * 1024;
    const CALLER_ALLOCATION_STACK_FRAMES: usize = 16;
    const CALLER_DEALLOCATION_STACK_FRAMES: usize = 16;
    const CALLER_TRACK_THREADS: bool = true;
    const CALLER_TRACK_HEAP_LIFETIMES: bool = true;
}

/// Defines an allocator [`Config`] as a zero-sized type.
///
/// # Options
///
/// | Option | Default | Description |
/// | --- | --- | --- |
/// | `track_aggregates` | `false` | Records aggregate statistics. |
/// | `track_callers` | `false` | Enables caller tracing support. |
/// | `caller_event_capacity` | `131072` | Retained caller events per thread; must be a power of two. |
/// | `caller_allocation_stack_frames` | `16` | Captured allocation stack frames, through 24. |
/// | `caller_deallocation_stack_frames` | `16` | Captured deallocation stack frames, through 24. |
/// | `caller_track_threads` | `true` | Records allocation and deallocation thread identities. |
/// | `caller_track_heap_lifetimes` | `true` | Retains heap identity and whether a free occurred after release; heap kind is always recorded. |
/// | `tunables` | [`crate::tunables::Standard`] | Selects allocator tunables. |
///
/// ```
/// use rallocator::{config, tunable};
///
/// tunable!(MyTunables {
///     partial_slab_scan_limit: 8,
///     recycled_bitmap_batch_max_block_size: 512,
///     medium_purge_delay_ms: 2_000,
/// });
///
/// config!(MyConfig {
///     track_aggregates: true,
///     track_callers: true,
///     tunables: MyTunables,
/// });
/// ```
#[macro_export]
macro_rules! config {
    (
        $visibility:vis $name:ident { $($options:tt)* }
    ) => {
        $crate::config!(
            @parse
            [$visibility]
            [$name]
            [false]
            [false]
            [128 * 1024]
            [16]
            [16]
            [true]
            [true]
            [$crate::tunables::Standard]
            $($options)*
        );
    };
    (
        @parse
        [$visibility:vis]
        [$name:ident]
        [$track_aggregates:expr]
        [$track_callers:expr]
        [$caller_event_capacity:expr]
        [$caller_allocation_stack_frames:expr]
        [$caller_deallocation_stack_frames:expr]
        [$caller_track_threads:expr]
        [$caller_track_heap_lifetimes:expr]
        [$tunables:ty]
    ) => {
        $visibility struct $name;

        impl $crate::config::Config for $name {
            type Tunables = $tunables;

            const TRACK_AGGREGATES: bool = $track_aggregates;
            const TRACK_CALLERS: bool = $track_callers;
            const CALLER_EVENT_CAPACITY: usize = $caller_event_capacity;
            const CALLER_ALLOCATION_STACK_FRAMES: usize = $caller_allocation_stack_frames;
            const CALLER_DEALLOCATION_STACK_FRAMES: usize = $caller_deallocation_stack_frames;
            const CALLER_TRACK_THREADS: bool = $caller_track_threads;
            const CALLER_TRACK_HEAP_LIFETIMES: bool = $caller_track_heap_lifetimes;
        }
    };
    (
        @parse
        [$visibility:vis] [$name:ident]
        [$track_aggregates:expr]
        [$track_callers:expr]
        [$caller_event_capacity:expr]
        [$caller_allocation_stack_frames:expr]
        [$caller_deallocation_stack_frames:expr]
        [$caller_track_threads:expr]
        [$caller_track_heap_lifetimes:expr]
        [$tunables:ty]
        track_aggregates: $value:expr $(, $($rest:tt)*)?
    ) => {
        $crate::config!(
            @parse
            [$visibility] [$name]
            [$value]
            [$track_callers]
            [$caller_event_capacity]
            [$caller_allocation_stack_frames]
            [$caller_deallocation_stack_frames]
            [$caller_track_threads]
            [$caller_track_heap_lifetimes]
            [$tunables]
            $($($rest)*)?
        );
    };
    (
        @parse
        [$visibility:vis] [$name:ident]
        [$track_aggregates:expr]
        [$track_callers:expr]
        [$caller_event_capacity:expr]
        [$caller_allocation_stack_frames:expr]
        [$caller_deallocation_stack_frames:expr]
        [$caller_track_threads:expr]
        [$caller_track_heap_lifetimes:expr]
        [$tunables:ty]
        track_callers: $value:expr $(, $($rest:tt)*)?
    ) => {
        $crate::config!(
            @parse
            [$visibility] [$name]
            [$track_aggregates]
            [$value]
            [$caller_event_capacity]
            [$caller_allocation_stack_frames]
            [$caller_deallocation_stack_frames]
            [$caller_track_threads]
            [$caller_track_heap_lifetimes]
            [$tunables]
            $($($rest)*)?
        );
    };
    (
        @parse
        [$visibility:vis] [$name:ident]
        [$track_aggregates:expr]
        [$track_callers:expr]
        [$caller_event_capacity:expr]
        [$caller_allocation_stack_frames:expr]
        [$caller_deallocation_stack_frames:expr]
        [$caller_track_threads:expr]
        [$caller_track_heap_lifetimes:expr]
        [$tunables:ty]
        caller_event_capacity: $value:expr $(, $($rest:tt)*)?
    ) => {
        $crate::config!(
            @parse
            [$visibility] [$name]
            [$track_aggregates]
            [$track_callers]
            [$value]
            [$caller_allocation_stack_frames]
            [$caller_deallocation_stack_frames]
            [$caller_track_threads]
            [$caller_track_heap_lifetimes]
            [$tunables]
            $($($rest)*)?
        );
    };
    (
        @parse
        [$visibility:vis] [$name:ident]
        [$track_aggregates:expr]
        [$track_callers:expr]
        [$caller_event_capacity:expr]
        [$caller_allocation_stack_frames:expr]
        [$caller_deallocation_stack_frames:expr]
        [$caller_track_threads:expr]
        [$caller_track_heap_lifetimes:expr]
        [$tunables:ty]
        caller_allocation_stack_frames: $value:expr $(, $($rest:tt)*)?
    ) => {
        $crate::config!(
            @parse
            [$visibility] [$name]
            [$track_aggregates]
            [$track_callers]
            [$caller_event_capacity]
            [$value]
            [$caller_deallocation_stack_frames]
            [$caller_track_threads]
            [$caller_track_heap_lifetimes]
            [$tunables]
            $($($rest)*)?
        );
    };
    (
        @parse
        [$visibility:vis] [$name:ident]
        [$track_aggregates:expr]
        [$track_callers:expr]
        [$caller_event_capacity:expr]
        [$caller_allocation_stack_frames:expr]
        [$caller_deallocation_stack_frames:expr]
        [$caller_track_threads:expr]
        [$caller_track_heap_lifetimes:expr]
        [$tunables:ty]
        caller_deallocation_stack_frames: $value:expr $(, $($rest:tt)*)?
    ) => {
        $crate::config!(
            @parse
            [$visibility] [$name]
            [$track_aggregates]
            [$track_callers]
            [$caller_event_capacity]
            [$caller_allocation_stack_frames]
            [$value]
            [$caller_track_threads]
            [$caller_track_heap_lifetimes]
            [$tunables]
            $($($rest)*)?
        );
    };
    (
        @parse
        [$visibility:vis] [$name:ident]
        [$track_aggregates:expr]
        [$track_callers:expr]
        [$caller_event_capacity:expr]
        [$caller_allocation_stack_frames:expr]
        [$caller_deallocation_stack_frames:expr]
        [$caller_track_threads:expr]
        [$caller_track_heap_lifetimes:expr]
        [$tunables:ty]
        caller_track_threads: $value:expr $(, $($rest:tt)*)?
    ) => {
        $crate::config!(
            @parse
            [$visibility] [$name]
            [$track_aggregates]
            [$track_callers]
            [$caller_event_capacity]
            [$caller_allocation_stack_frames]
            [$caller_deallocation_stack_frames]
            [$value]
            [$caller_track_heap_lifetimes]
            [$tunables]
            $($($rest)*)?
        );
    };
    (
        @parse
        [$visibility:vis] [$name:ident]
        [$track_aggregates:expr]
        [$track_callers:expr]
        [$caller_event_capacity:expr]
        [$caller_allocation_stack_frames:expr]
        [$caller_deallocation_stack_frames:expr]
        [$caller_track_threads:expr]
        [$caller_track_heap_lifetimes:expr]
        [$tunables:ty]
        caller_track_heap_lifetimes: $value:expr $(, $($rest:tt)*)?
    ) => {
        $crate::config!(
            @parse
            [$visibility] [$name]
            [$track_aggregates]
            [$track_callers]
            [$caller_event_capacity]
            [$caller_allocation_stack_frames]
            [$caller_deallocation_stack_frames]
            [$caller_track_threads]
            [$value]
            [$tunables]
            $($($rest)*)?
        );
    };
    (
        @parse
        [$visibility:vis] [$name:ident]
        [$track_aggregates:expr]
        [$track_callers:expr]
        [$caller_event_capacity:expr]
        [$caller_allocation_stack_frames:expr]
        [$caller_deallocation_stack_frames:expr]
        [$caller_track_threads:expr]
        [$caller_track_heap_lifetimes:expr]
        [$tunables:ty]
        tunables: $value:ty $(, $($rest:tt)*)?
    ) => {
        $crate::config!(
            @parse
            [$visibility] [$name]
            [$track_aggregates]
            [$track_callers]
            [$caller_event_capacity]
            [$caller_allocation_stack_frames]
            [$caller_deallocation_stack_frames]
            [$caller_track_threads]
            [$caller_track_heap_lifetimes]
            [$value]
            $($($rest)*)?
        );
    };
}
