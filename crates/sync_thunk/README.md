<div align="center">
 <img src="./logo.png" alt="Sync Thunk Logo" width="96">

# Sync Thunk

[![crate.io](https://img.shields.io/crates/v/sync_thunk.svg)](https://crates.io/crates/sync_thunk)
[![docs.rs](https://docs.rs/sync_thunk/badge.svg)](https://docs.rs/sync_thunk)
[![MSRV](https://img.shields.io/crates/msrv/sync_thunk)](https://crates.io/crates/sync_thunk)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Efficiently handle blocking calls in async code.

Mark any async method with [`#[thunk]`][__link0] and its body will execute on a
dedicated worker thread, freeing the async executor to do other work.

```rust
use sync_thunk::{Thunker, thunk};

struct MyService {
    thunker: Thunker,
}

impl MyService {
    #[thunk(from = self.thunker)]
    async fn blocking_work(&self) -> String {
        // This body runs on a worker thread, not the async executor.
        std::fs::read_to_string("/etc/hostname").unwrap()
    }
}
```

## Why

Async runtimes assume tasks yield quickly. Blocking operations — filesystem I/O,
DNS lookups, CPU-heavy computation — stall the executor and hurt throughput.
The traditional fix is `spawn_blocking`, but that allocates a closure, boxes the
return value, and may spawn an unbounded number of OS threads.

`sync_thunk` solves this differently:

* **Zero-allocation dispatch.** Arguments are packed into a stack-allocated struct
  and sent through a pre-allocated bounded channel. No `Box`, no `Arc`, no closure.

* **Zero-copy design.** Arguments is moved to the worker thread without requiring any copying or funny ownership gymnastics.

* **Auto-scaling thread pool.** The [`Thunker`][__link1] starts with a single worker thread
  and automatically scales up when the queue backs up — up to a configurable
  maximum. Idle workers exit after a configurable cool-down interval, but at least
  one worker is always kept alive.

## Getting Started

**1. Create a [`Thunker`][__link2]:**

```rust
use sync_thunk::Thunker;

let thunker = Thunker::builder()
    .max_thread_count(4) // at most 4 workers
    .cool_down_interval(std::time::Duration::from_secs(10))
    .build();
```

**2. Annotate methods with [`#[thunk]`][__link3]:**

The `from` parameter tells the macro where to find the [`Thunker`][__link4]. It can be a
struct field, a method call, a function parameter, or a static — anything that returns a `&Thunker`.

```rust
#[thunk(from = self.thunker)]
async fn do_io(&self) -> std::io::Result<Vec<u8>> {
    std::fs::read("/some/file")
}
```

**3. Call it like any other async method:**

```rust
let data = service.do_io().await?;
```

## Where the Thunker Comes From

The `from` parameter is flexible. Here are the four common patterns:

### From a struct field

The most common pattern — the struct owns the thunker:

```rust
struct MyService { thunker: Thunker }

impl MyService {
    #[thunk(from = self.thunker)]
    async fn work(&self) -> u64 { /* ... */ }
}
```

### From a method call

Useful when the thunker is behind a getter or shared via an accessor:

```rust
impl MyService {
    fn thunker(&self) -> &Thunker { &self.inner_thunker }

    #[thunk(from = self.thunker())]
    async fn work(&self) -> u64 { /* ... */ }
}
```

### From a function parameter

Useful for associated functions with no `self` receiver:

```rust
impl MyService {
    #[thunk(from = thunker)]
    async fn create(thunker: &Thunker, path: &Path) -> std::io::Result<Self> {
        let data = std::fs::read(path)?;
        /* ... */
    }
}
```

### From a global static

For applications that share a single pool without threading it through structs:

```rust
static THUNKER: LazyLock<Thunker> = LazyLock::new(|| Thunker::builder().build());

impl MyService {
    #[thunk(from = THUNKER)]
    async fn work(&self) -> u64 { /* ... */ }
}
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/sync_thunk">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG_BuYsgMfBazG9TJGWvNurCDGwRx9icGpbylGxSUKrXYyeSHYWSBgmpzeW5jX3RodW5rZTAuMS4w
 [__link0]: https://docs.rs/sync_thunk/0.1.0/sync_thunk/?search=thunk
 [__link1]: https://docs.rs/sync_thunk/0.1.0/sync_thunk/?search=Thunker
 [__link2]: https://docs.rs/sync_thunk/0.1.0/sync_thunk/?search=Thunker
 [__link3]: https://docs.rs/sync_thunk/0.1.0/sync_thunk/?search=thunk
 [__link4]: https://docs.rs/sync_thunk/0.1.0/sync_thunk/?search=Thunker
