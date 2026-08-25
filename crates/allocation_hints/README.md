<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Allocation Hints Logo" width="96">

# Allocation Hints

[![crate.io](https://img.shields.io/crates/v/allocation_hints.svg)](https://crates.io/crates/allocation_hints)
[![docs.rs](https://docs.rs/allocation_hints/badge.svg)](https://docs.rs/allocation_hints)
[![MSRV](https://img.shields.io/crates/msrv/allocation_hints)](https://crates.io/crates/allocation_hints)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Infrastructure for allocator-agnostic heap allocation hints.

Libraries can use [`with_hint`][__link0] to set a thread-local [`Hint`][__link1] that a
supporting global allocator can query during allocation.

This lets users direct the allocator to prefer a particular heap or allocation
strategy, potentially improving performance and locality while reducing long-term
fragmentation.

The underlying model is structured into domains, heaps, and hints. A **domain** is a set of
equal-sized, low-level “dumb” allocation regions obtained from the operating system. Allocators
internally divide these into subregions that heaps can reserve and return. A **heap** then
manages its assigned subregions according to its preferred strategy. One type of heap might
optimize for fixed size classes, another might treat its regions as bump space, and yet another
might act as a *general-purpose* heap using mixed-mode allocation.

At the lowest level, **hints** are ad hoc instructions to the current thread’s allocator to
prefer a particular heap. However, hints always remain advisory. Allocators that do not support
them may ignore them, while preserving all existing allocation API contracts.

## Example

This requires a supporting global allocator backend to be registered first.

```rust
use allocation_hints::heap::{Heap, bump};
use allocation_hints::with_hint;

let heap = Heap::bump(bump::Options::new());
with_hint(&heap, || {
    let values = vec![1, 2, 3];
    assert_eq!(values.len(), 3);
});
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/allocation_hints">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQb11VxC_uAPOQbtUn4Wx2-BfAbid3Nt1Y27Pobprn8Z6FjFy9hYvRhcoQbYL9YWZaDL3Mb79z5R8VjMvwbof14VP8_iP8bmFzkKlHqi6dhZIGCcGFsbG9jYXRpb25faGludHNlMC4xLjA
 [__link0]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/fn.with_hint.html
 [__link1]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/struct.Hint.html
