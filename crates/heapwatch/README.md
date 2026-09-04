<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Heapwatch Logo" width="96">

# Heapwatch

[![crate.io](https://img.shields.io/crates/v/heapwatch.svg)](https://crates.io/crates/heapwatch)
[![docs.rs](https://docs.rs/heapwatch/badge.svg)](https://docs.rs/heapwatch)
[![MSRV](https://img.shields.io/crates/msrv/heapwatch)](https://crates.io/crates/heapwatch)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

A global allocator wrapper that continuously accounts heap usage.

Heapwatch wraps another allocator and, installed as a binary’s
`#[global_allocator]`, reports how many bytes that binary’s Rust heap holds
and how much has moved through it. It answers *how much*, never *who* — no
backtraces, no per-allocation metadata, no pointer-to-size shadow map —
which is what keeps the per-allocation cost to a few non-atomic adds, low
enough to leave enabled in production.

## Status

**The API is not implemented yet — this crate is currently scaffolding.**
The architecture is settled and written up in [`DESIGN.md`][__link0], and the ideas
deliberately left out of it are recorded in [`TODO.md`][__link1]. Both land ahead of
the code so the design can be reviewed on its own terms.

## The mechanism

Each thread accumulates its own totals with plain, non-atomic arithmetic and
publishes them into the allocator instance’s atomic totals in batches — once
*bytes allocated plus bytes freed* since its last publication crosses a
compile-time threshold, and again when the thread exits. That removes the
atomic read-modify-write per allocation that an exact accounting wrapper
pays, which is the cost that scales badly with core count.

The trade is a small, bounded, stated inaccuracy: a reading omits whatever
each live thread has not yet published, at most the threshold times the
number of live threads. That bound depends on neither the allocation rate
nor how long the process has been running. Reading is O(1) in the thread
count — a handful of relaxed loads through a handle obtained from the
allocator, with no registry to walk.

## Measurement boundary

Heapwatch counts successful calls through the registered `GlobalAlloc`, and
counts them as *requested* rather than as reserved, since the trait has no
hook to ask the inner allocator what it actually rounded up to. Allocations
that never reach the global allocator — native and FFI heaps, direct OS
mappings, anything routed to `std::alloc::System` — are outside the
boundary, as are thread stacks, static data, and the inner allocator’s own
metadata and fragmentation. It therefore complements, rather than replaces,
allocator-native statistics and process-level metrics such as resident set
size.


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/heapwatch">source code</a>.
</sub>

 [__link0]: https://github.com/microsoft/oxidizer/blob/main/crates/heapwatch/docs/DESIGN.md
 [__link1]: https://github.com/microsoft/oxidizer/blob/main/crates/heapwatch/docs/TODO.md
