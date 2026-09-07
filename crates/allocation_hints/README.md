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

Passive scoped allocation hints.

This crate owns logical heap descriptors and the current thread’s requested
descriptor. It does not register, call, or query an allocator backend.
Supporting global allocators may inspect [`active_hint`][__link0] and lazily realize
the requested heap. Other allocators simply ignore the hint.

```rust
use allocation_hints::heaps::{Heap, bump};
use allocation_hints::with_hint;

let heap = Heap::bump(bump::Options::new());
let values = with_hint(&heap, || vec![1, 2, 3]);
assert_eq!(values.len(), 3);
```

[`heaps::thread_heap`][__link1] captures the current thread’s logical identity. A
supporting allocator associates that identity with the heap it would
ordinarily use for the thread, allowing another thread to allocate toward
that owner:

```rust
use allocation_hints::heaps::thread_heap;
use allocation_hints::with_hint;

let owner = thread_heap();
let value = std::thread::spawn(move || with_hint(&owner, || Box::new(42)))
    .join()
    .unwrap();
assert_eq!(*value, 42);
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/allocation_hints">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEG9dVcQv7gDzkG7VJ-FsdvgXwG4ndzbdWNuz6G6a5_GehYxcvYXKEGyuR42uyBFsLG75s4FnZBDpcG6dl7lBCyuwuG0QO3Is4E_5qYWSBgnBhbGxvY2F0aW9uX2hpbnRzZTAuMS4w
 [__link0]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/fn.active_hint.html
 [__link1]: https://docs.rs/allocation_hints/0.1.0/allocation_hints/?search=heaps::thread_heap
