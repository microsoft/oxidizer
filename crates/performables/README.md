<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Performables Logo" width="96">

# Performables

[![crate.io](https://img.shields.io/crates/v/performables.svg)](https://crates.io/crates/performables)
[![docs.rs](https://docs.rs/performables/badge.svg)](https://docs.rs/performables)
[![MSRV](https://img.shields.io/crates/msrv/performables)](https://crates.io/crates/performables)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Performance-oriented ownership and synchronization primitives.

The synchronization types are executor-independent and optimize for the
uncontended case. Acquiring an available lock does not allocate; async waiter
storage is allocated lazily after contention is observed and reused
thereafter. Mutexes, reader-writer locks, barriers, and condition variables
default to compact [`sync::mode::Sync`][__link0] storage. Select [`sync::mode::Async`][__link1]
when the same object must support asynchronous waits as well as blocking
callers. Synchronous operations use unsuffixed names such as
[`lock`][__link2], while asynchronous operations are explicit,
such as [`lock_async`][__link3]. This convention also
applies to reader-writer locks, waits, and channels.
Default lock acquisition panics on poison, while explicit `*_result`
APIs return [`sync::PoisonError`][__link4] with the acquired guard for recovery.
[`sync::barrier::Barrier`][__link5] and [`sync::condition::Condvar`][__link6] follow the same
mode selection, while [`sync::once::OnceLock`][__link7] and
[`sync::once::LazyLock`][__link8] instrument one-time initialization.
[`sync::channel`][__link9] provides multi-producer queues, oneshot transfer, and
independently versioned latest-value observation. The optional `seismograph`
feature enables runtime ownership and synchronization telemetry.

[`arc::Arc`][__link10] defaults to a process-wide allocation with the same
representation size as [`std::sync::Arc`][__link11]. Its thread-aware per-thread and
per-NUMA strategies lazily materialize and reuse affinity-local values.

## Choosing a synchronization mode

The mode belongs to the shared object, not to individual callers.
Blocking and asynchronous acquisitions of an async-capable lock coordinate
through the same ownership state. Synchronous guards follow the native
backend’s thread-affinity requirements and must not be held across an await.
Mode selection adds no identity field: telemetry continues to identify each
lock, barrier, or condition variable by its outer object’s address, not its
backend or lazily allocated wait queue. As before, moving an object changes
that identity, and an address can be reused after an object is dropped.

```rust
use performables::sync::mode;
use performables::sync::mutex::Mutex;

let counter = Mutex::<u64>::new(0);
*counter.lock() += 1;
assert_eq!(*counter.lock(), 1);

let shared = Mutex::<u64, mode::Async>::new(0);
*shared.lock() += 1;
let acquire = shared.lock_async();
// A pending acquisition does not take ownership until it is polled.
drop(acquire);
assert_eq!(*shared.lock(), 1);
```

Async methods are intentionally unavailable on synchronous objects:

```rust
use performables::sync::mutex::Mutex;
let mutex = Mutex::<u64>::new(0);
let _future = mutex.lock_async();
```

```rust
use performables::sync::lock::RwLock;
let lock = RwLock::<u64>::new(0);
let _future = lock.read_async();
```

```rust
use performables::sync::{condition::Condvar, mode, mutex::Mutex};
let mutex = Mutex::<u64>::new(0);
let condition = Condvar::<mode::Sync>::new();
let _future = condition.wait_async(mutex.lock());
```

```rust
use performables::sync::{barrier::Barrier, mode};
let barrier = Barrier::<mode::Sync>::new(1);
let _future = barrier.wait_async();
```

Native synchronous guards cannot be transferred to another thread:

```rust
use performables::sync::mutex::Mutex;
fn require_send<T: Send>(_: T) {}
let mutex = Mutex::<u64>::new(0);
require_send(mutex.lock());
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/performables">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbXtvRAuEVsrEbGCTGty00QhgbrMfHmb1_2HgbGQZYcIW5oaFhZIGCbHBlcmZvcm1hYmxlc2UwLjIuMA
 [__link0]: https://docs.rs/performables/0.2.0/performables/?search=sync::mode::Sync
 [__link1]: https://docs.rs/performables/0.2.0/performables/?search=sync::mode::Async
 [__link10]: https://docs.rs/performables/0.2.0/performables/?search=arc::Arc
 [__link11]: https://doc.rust-lang.org/stable/std/?search=sync::Arc
 [__link2]: https://docs.rs/performables/0.2.0/performables/?search=sync::mutex::Mutex::lock
 [__link3]: https://docs.rs/performables/0.2.0/performables/?search=sync::mutex::Mutex::lock_async
 [__link4]: https://docs.rs/performables/0.2.0/performables/?search=sync::PoisonError
 [__link5]: https://docs.rs/performables/0.2.0/performables/?search=sync::barrier::Barrier
 [__link6]: https://docs.rs/performables/0.2.0/performables/?search=sync::condition::Condvar
 [__link7]: https://docs.rs/performables/0.2.0/performables/?search=sync::once::OnceLock
 [__link8]: https://docs.rs/performables/0.2.0/performables/?search=sync::once::LazyLock
 [__link9]: https://docs.rs/performables/0.2.0/performables/?search=sync::channel
