// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![expect(
    missing_debug_implementations,
    missing_docs,
    reason = "The initial allocator and telemetry surfaces remain intentionally unstable while their public shape is refined"
)]
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Allocator dimensions and wire fields are range-checked by their surrounding layout invariants"
)]
#![expect(
    clippy::cast_ptr_alignment,
    reason = "Raw mappings are explicitly aligned before typed allocator metadata is constructed"
)]
#![expect(clippy::inline_always, reason = "Selected allocator fast paths intentionally force inlining")]
#![expect(
    clippy::manual_let_else,
    reason = "The existing forms mirror allocator state-machine and bitmap operations"
)]
#![expect(
    clippy::missing_errors_doc,
    reason = "Internal configuration and telemetry types are documented through their containing API"
)]
#![expect(
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    reason = "Unsafe regions implement module-level allocator invariants documented at their helper and type boundaries"
)]
#![expect(
    clippy::needless_pass_by_value,
    clippy::needless_pass_by_ref_mut,
    clippy::unused_self,
    reason = "Signatures intentionally preserve uniform allocator and benchmark callback shapes"
)]
#![expect(
    clippy::non_send_fields_in_send_ty,
    reason = "Raw pointers reference process-retained or explicitly synchronized allocator state"
)]
#![expect(
    clippy::renamed_function_params,
    reason = "Implementation parameter names are clearer than generic trait names"
)]
#![expect(clippy::struct_field_names, reason = "Telemetry units stay explicit at call sites")]
#![expect(
    clippy::too_many_lines,
    reason = "Allocator state-machine functions remain linear so lock and ownership transitions are auditable"
)]
#![expect(
    clippy::unwrap_used,
    reason = "Infallible formatting and test-only invariant checks use unwrap to expose programming errors"
)]
#![cfg_attr(
    test,
    expect(
        clippy::clone_on_ref_ptr,
        clippy::unnecessary_wraps,
        reason = "Tests intentionally materialize ownership and mirror fallible callback signatures"
    )
)]
#![cfg_attr(
    all(test, feature = "tuning-telemetry"),
    expect(clippy::iter_with_drain, reason = "The test explicitly drains before reverse-order deallocation")
)]

//! A pure-Rust, high-performance allocator integrated with [`allocation_hints`].
//!
//! # Supported platforms
//!
//! `rallocator` currently supports Windows and Linux. Other operating systems
//! are outside the crate's public support contract and intentionally fail to
//! compile. Miri uses an internal test backend and is not a production support
//! target.
//!
//! # Usage
//!
//! ## General use
//!
//! Install the standard configuration as the process-global allocator:
//!
//! ```
//! rallocator::rallocator!();
//! rallocator::initialize();
//! ```
//!
//! The macro declares the required `#[global_allocator]` static and contains
//! the unsafe call to [`Rallocator::new`], whose process-wide
//! same-configuration invariant it establishes by construction. [`initialize`]
//! registers rallocator with [`allocation_hints`] before heap or domain APIs are
//! used.
//!
//! Ordinary allocations then use each thread's implicit general heap. To group
//! related allocations, create a [`Heap`](allocation_hints::heap::Heap) and
//! activate it for a synchronous scope:
//!
//! ```
//! use allocation_hints::heap::{Heap, bump};
//! use allocation_hints::with_hint;
//!
//! rallocator::rallocator!();
//! rallocator::initialize();
//!
//! let heap = Heap::bump(bump::Options::new());
//! let values = with_hint(&heap, || vec![1, 2, 3]);
//! assert_eq!(values.len(), 3);
//! ```
//!
//! ## Telemetry
//!
//! Telemetry is opt-in at compile time. Define a [`config::Config`] with
//! aggregate or caller tracking enabled, then pass it to [`rallocator!`]:
//!
//! ```no_run
//! use rallocator::telemetry::{snapshot, stats, track_callers};
//!
//! rallocator::config!(Telemetry {
//!     track_aggregates: true,
//!     track_callers: true,
//! });
//! rallocator::rallocator!(Telemetry);
//!
//! fn main() -> std::io::Result<()> {
//!     rallocator::initialize();
//!     track_callers(true);
//!     let mut values = vec![1, 2, 3];
//!     values[0] += 1;
//!     std::hint::black_box(&values);
//!     drop(values);
//!     track_callers(false);
//!
//!     snapshot()
//!         .expect("telemetry tracking is enabled")
//!         .write_file("snapshot.rallocator")?;
//!
//!     Ok(())
//! }
//! ```
//!
//! Convert the snapshot to HTML with `rallocator_cli snapshot html snapshot.rallocator`.
//!
//! Aggregate counters remain active for the process lifetime. Caller tracking
//! can be enabled only around the interval of interest to limit its overhead;
//! snapshots include allocator topology, counters, and retained caller events.
//! The default `caller-symbolization` feature resolves captured instruction
//! pointers through the optional `backtrace` dependency. Disabling default
//! features retains caller tracking and raw addresses without in-process symbol
//! resolution.
//!
//! # Design guide
//!
//! Use the implicit thread-local general heap for ordinary allocations.
//! Introduce explicit general heaps when you want locality boundaries without
//! changing the mixed-size allocation model. Use bump heaps for phase-bounded
//! work where individual frees are rare and bulk reclamation matters more than
//! per-allocation reuse. Use separate [`Domain`](allocation_hints::domain::Domain)
//! values only when heaps must stop sharing allocator-managed regions.
//!
//! # Implementation guide
//!
//! 1. Choose a [`config::Config`] and, if needed, custom
//!    [`tunables::Tunables`].
//! 2. Install [`rallocator!`] exactly once as the process-global allocator.
//! 3. Call [`initialize`] near the start of `main` before creating
//!    allocation-hint heaps or domains.
//! 4. Route special-purpose allocations with
//!    [`allocation_hints::with_hint`] and [`allocation_hints::heap::Heap`].
//! 5. Enable telemetry only when you need it: aggregate counters are a
//!    compile-time choice, while caller tracking is also runtime-gated.
//!
//! Invalid tunables fail early when [`Rallocator::new`] is instantiated: the
//! size-class layout must be well-formed and the partial-slab scan limit must
//! be non-zero.
//!
//! # Internals
//!
//! Rallocator is organized as a hierarchy:
//!
//! - A [`Domain`](allocation_hints::domain::Domain) owns one or more 1 GiB virtual-memory
//!   **regions**, divided into 64 KiB **slices**.
//! - A domain can serve multiple [`Heap`](allocation_hints::heap::Heap) instances.
//!   Each heap keeps its own allocation state while drawing backing memory from
//!   its domain.
//! - General heaps use slices for **locality segments** containing 32 KiB
//!   **slabs**, and for **medium spans** covering one or more slices. Bump heaps
//!   use slices as **bump chunks**.
//! - Slabs contain same-sized **blocks**, which are the allocation slots returned
//!   for small requests. Large or highly aligned requests bypass this hierarchy
//!   and use direct operating-system mappings.
//!
//! <div style="overflow-x: auto">
//! <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1100 680"
//!      role="img" aria-labelledby="allocator-layout-title"
//!      style="width: 100%; min-width: 820px; max-width: 1100px">
//!   <title id="allocator-layout-title">Allocator memory layout</title>
//!   <g fill="none" stroke="currentColor" stroke-width="2">
//!     <rect x="15" y="15" width="1070" height="650" rx="10"/>
//!     <rect x="40" y="60" width="775" height="410" rx="8"/>
//!     <rect x="840" y="60" width="220" height="160" rx="8"/>
//!     <rect x="840" y="245" width="220" height="160" rx="8"/>
//!     <rect x="65" y="110" width="725" height="75" rx="5"/>
//!     <line x1="246" y1="110" x2="246" y2="185"/>
//!     <line x1="427" y1="110" x2="427" y2="185"/>
//!     <line x1="608" y1="110" x2="608" y2="185"/>
//!     <rect x="65" y="220" width="350" height="120" rx="5"/>
//!     <rect x="80" y="280" width="150" height="45" rx="4"/>
//!     <rect x="250" y="280" width="150" height="45" rx="4"/>
//!     <rect x="440" y="220" width="350" height="120" rx="5"/>
//!     <line x1="615" y1="275" x2="615" y2="340"/>
//!     <rect x="65" y="370" width="725" height="70" rx="5"/>
//!     <line x1="427.5" y1="370" x2="427.5" y2="440"/>
//!     <rect x="65" y="495" width="725" height="150" rx="8"/>
//!     <rect x="85" y="550" width="95" height="50" rx="3"/>
//!     <rect x="180" y="550" width="95" height="50" rx="3"/>
//!     <rect x="275" y="550" width="95" height="50" rx="3"/>
//!     <rect x="370" y="550" width="95" height="50" rx="3"/>
//!     <rect x="465" y="550" width="95" height="50" rx="3"/>
//!     <rect x="560" y="550" width="95" height="50" rx="3"/>
//!     <rect x="655" y="550" width="115" height="50" rx="3"/>
//!   </g>
//!   <g fill="currentColor" font-family="sans-serif">
//!     <text x="35" y="43" font-size="18" font-weight="bold">Domain-owned physical backing</text>
//!     <text x="60" y="88" font-size="17" font-weight="bold">Process region: 1 GiB</text>
//!     <text x="75" y="138" font-size="14">64 KiB slice</text>
//!     <text x="256" y="138" font-size="14">64 KiB slice</text>
//!     <text x="437" y="138" font-size="14">64 KiB slice</text>
//!     <text x="618" y="138" font-size="14">64 KiB slice</text>
//!     <text x="65" y="205" font-size="12">A bitmap records which slices are owned.</text>
//!     <text x="80" y="246" font-size="16" font-weight="bold">Locality segment</text>
//!     <text x="80" y="267" font-size="12">Consecutive slices owned by one general heap</text>
//!     <text x="115" y="308" font-size="14">32 KiB slab</text>
//!     <text x="285" y="308" font-size="14">32 KiB slab</text>
//!     <text x="455" y="246" font-size="16" font-weight="bold">Medium span</text>
//!     <text x="455" y="267" font-size="12">One or more consecutive slices</text>
//!     <text x="500" y="310" font-size="14">slice 1</text>
//!     <text x="675" y="310" font-size="14">slice 2</text>
//!     <text x="80" y="397" font-size="16" font-weight="bold">Bump chunk</text>
//!     <text x="80" y="420" font-size="12">One 64 KiB slice</text>
//!     <text x="250" y="412" font-size="14">32 KiB segment</text>
//!     <text x="520" y="412" font-size="14">32 KiB segment</text>
//!     <text x="860" y="88" font-size="16" font-weight="bold">Direct mapping</text>
//!     <text x="860" y="118" font-size="12">One large or highly</text>
//!     <text x="860" y="137" font-size="12">aligned allocation.</text>
//!     <text x="860" y="170" font-size="12">Managed independently</text>
//!     <text x="860" y="189" font-size="12">by the operating system.</text>
//!     <text x="860" y="273" font-size="16" font-weight="bold">Additional region</text>
//!     <text x="860" y="303" font-size="12">Created when existing</text>
//!     <text x="860" y="322" font-size="12">regions cannot provide</text>
//!     <text x="860" y="341" font-size="12">a large enough free run.</text>
//!     <text x="860" y="374" font-size="12">Uses the same layout.</text>
//!     <text x="80" y="522" font-size="16" font-weight="bold">A slab contains blocks from one size class</text>
//!     <text x="110" y="580" font-size="12">header</text>
//!     <text x="208" y="580" font-size="12">block</text>
//!     <text x="303" y="580" font-size="12">block</text>
//!     <text x="398" y="580" font-size="12">block</text>
//!     <text x="493" y="580" font-size="12">block</text>
//!     <text x="588" y="580" font-size="12">block</text>
//!     <text x="690" y="580" font-size="12">...</text>
//!     <text x="85" y="625" font-size="12">Example: every block in this slab is the selected 64-byte size class.</text>
//!     <text x="930" y="642" font-size="11">Not to scale</text>
//!   </g>
//! </svg>
//! </div>
//!
//! Each thread has an implicit general heap. [`with_hint`](allocation_hints::with_hint) can
//! temporarily route allocations to an explicit general or bump heap instead;
//! leaving the scope restores the previous heap. This keeps the common
//! allocation path thread-local while allowing runtimes and data structures to
//! choose allocation topology deliberately.
//!
//! General heaps route requests by size and alignment:
//!
//! | Category | Size | Maximum alignment | Backing |
//! | --- | --- | --- | --- |
//! | **Small** | Up to 16 KiB | 4 KiB | One block in a 32 KiB slab |
//! | **Medium** | Up to 1 GiB, when no small class fits | 64 KiB | One or more 64 KiB slices |
//! | **Large/direct** | Above 1 GiB, or alignment above 64 KiB | Operating-system limit | Dedicated mapping |
//!
//! Small frees normally return to the owning heap's caches; cross-thread frees
//! are queued for that owner. Medium spans are cached or returned to domain
//! free lists, while direct mappings go back to the operating system. Dropping
//! an explicit heap releases empty backing memory but preserves metadata for any
//! allocations that escaped its lifetime.

mod allocator;
pub mod config;
mod domain;
mod hal;
mod heap;
pub mod telemetry;
pub mod tunables;
#[cfg(feature = "tuning-telemetry")]
#[cfg_attr(not(test), expect(dead_code, reason = "internal tuning diagnostics are exercised by crate tests"))]
mod tuning_telemetry;

pub use allocator::Rallocator;

/// Registers rallocator as the process allocation-hint backend.
///
/// Call this once near the start of `main`, after declaring the global
/// allocator with [`rallocator!`], and before creating allocation-hint heaps or
/// domains. Repeated calls are harmless.
///
/// Ordinary global allocation does not require initialization and does not
/// implicitly install the allocation-hint backend.
pub fn initialize() {
    heap::ensure_backend_registered();
}

/// Installs the process-global allocator.
///
/// The optional argument selects a [`config::Config`], including its tunables.
/// Omitting it uses [`config::Standard`].
///
/// ```
/// rallocator::rallocator!();
/// rallocator::initialize();
/// ```
#[macro_export]
macro_rules! rallocator {
    () => {
        $crate::rallocator!($crate::config::Standard);
    };
    ($config:ty $(,)?) => {
        #[global_allocator]
        static GLOBAL: $crate::Rallocator<$config> = unsafe { $crate::Rallocator::new() };
    };
}

#[cfg(test)]
mod tests {
    #[test]
    fn initialization_is_idempotent() {
        crate::initialize();
        crate::initialize();
    }
}
