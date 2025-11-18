<div align="center">
 <img src="./logo.png" alt="Thread Aware Logo" width="128">

# Thread Aware

[![crate.io](https://img.shields.io/crates/v/thread_aware.svg)](https://crates.io/crates/thread_aware)
[![docs.rs](https://docs.rs/thread_aware/badge.svg)](https://docs.rs/thread_aware)
[![MSRV](https://img.shields.io/crates/msrv/thread_aware)](https://crates.io/crates/thread_aware)
[![CI](https://github.com/microsoft/oxidizer/workflows/main/badge.svg)](https://github.com/microsoft/oxidizer/actions)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

</div>

* [Summary](#summary)
* [Feature Flags](#feature-flags)
* [Example](#example)
* [Derive Macro Example](#derive-macro-example)

## Summary

<!-- cargo-rdme start -->

A library for creating isolated affinities in Rust, allowing for safe and efficient data transfer between them.

This is useful in scenarios where you want to isolate data to different affinities, such as NUMA nodes, threads, or specific CPU cores.
It can serve as a foundation for building a async runtime or a task scheduler that operates across multiple affinities.
For example it can be used to implement a NUMA-aware task scheduler that transfers data between different NUMA nodes
or a thread-per-core scheduler that transfers data between different CPU cores.

The way this would be used is by restricting how work can be scheduled on other affinities (threads,
NUMA nodes). If the runtime only allows work scheduling in a way that accepts work that can be
transferred (e.g. by using the [`RelocateFnOnce`] trait) and makes sure that transfer is called, it can
effectively isolate the affinities as the [`ThreadAware`] trait ensures the right level of separation if
implemented correctly.

`ThreadAware` is an 'infectious' trait, meaning that when you implement it for a type,
all of its fields must also implement [`ThreadAware`] and you must call their `transfer` methods.
However [`ThreadAware`] is provided for many common types, so you can use it out of the box for most cases.

## Feature Flags
* **`derive`** *(default)* – Re-exports the `#[derive(ThreadAware)]` macro from the companion
  `thread_aware_macros` crate. Disable to avoid pulling in proc-macro code in minimal
  environments: `default-features = false`.

## Examples

```rust
use thread_aware::{MemoryAffinity, ThreadAware, Unaware, create_manual_affinities};

// Define a type that implements ThreadAware
#[derive(Debug, Clone)]
struct MyData {
    value: i32,
}

impl ThreadAware for MyData {
    fn relocated(mut self, source: MemoryAffinity, destination: MemoryAffinity) -> Self {
        self.value = self.value.relocated(source, destination);
        self
    }
}

fn do_transfer() {
    // Create two affinities
    let affinities = create_manual_affinities(&[2]);

    // Create an instance of MyData
    let data = MyData { value: 42 };

    // Transfer data from one affinity to another
    let transferred_data = data.relocated(affinities[0], affinities[1]);

    // Use Inert to create a type that does not transfer data
    struct MyInertData(i32);

    let inert_data = Unaware(MyInertData(100));
    let transferred_inert_data = inert_data.relocated(affinities[0], affinities[1]);
}
```

### Derive Macro Example

When the `derive` feature (enabled by default) is active you can simply
derive [`ThreadAware`] instead of writing the implementation manually.

```rust
use thread_aware::{ThreadAware, create_manual_affinities};

#[derive(Debug, Clone, ThreadAware)]
struct Point {
    x: i32,
    y: i32,
}

fn derived_example() {
    let affinities = create_manual_affinities(&[2]);
    let p = Point { x: 5, y: 9 };
    // Transfer the value between two affinities. In this simple case the
    // data just gets copied, but for complex types the generated impl
    // calls `transfer` on each field.
    let _p2 = p.relocated(affinities[0], affinities[1]);
}
```

If you disable default features (or the `derive` feature explicitly) you
can still implement [`ThreadAware`] manually as shown in the earlier example.

<!-- cargo-rdme end -->

<div style="font-size: 75%" ><hr/>

This crate was developed as part of [The Oxidizer Project](https://github.com/microsoft/oxidizer).

</div>
