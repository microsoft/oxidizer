<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Future-Assert Logo" width="96">

# Future-Assert

[![crate.io](https://img.shields.io/crates/v/future-assert.svg)](https://crates.io/crates/future-assert)
[![docs.rs](https://docs.rs/future-assert/badge.svg)](https://docs.rs/future-assert)
[![MSRV](https://img.shields.io/crates/msrv/future-assert)](https://crates.io/crates/future-assert)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Utilities for driving futures by hand in unit tests.

[`FutureTestExt`][__link0] adds consuming manual-polling assertions (`unwrap_ready`,
`unwrap_pending`, `unwrap_ready_within`, `unwrap_ready_after`,
`unwrap_pending_for`) that let a test poll a future a fixed number of times
and assert the outcome, without spinning up an async runtime. The
lower-level [`poll_once`][__link1] primitive performs exactly one poll of an unpinned
future.

## Usage

Add `future-assert` to your dev-dependencies, then import [`FutureTestExt`][__link2]:

```rust
use future_assert::FutureTestExt;

assert_eq!(async { 7 }.unwrap_ready(), 7);
std::future::pending::<()>().unwrap_pending();
```

## Polling behavior

Every poll uses a no-op waker. Repeated polls happen immediately, without
waiting for wakeups or elapsed time. Poll counts describe manual polling,
not executor scheduling, and these helpers do not verify waking behavior.

The assertion methods consume and drop the supplied future. To keep a
future between assertions, pin it and pass a mutable pinned reference:

```rust
use std::cell::Cell;
use std::future::poll_fn;
use std::pin::pin;
use std::task::Poll;

use future_assert::FutureTestExt;

let ready = Cell::new(false);
let mut future = pin!(poll_fn(|_| {
    if ready.get() {
        Poll::Ready(7)
    } else {
        Poll::Pending
    }
}));

future.as_mut().unwrap_pending();
ready.set(true);
assert_eq!(future.as_mut().unwrap_ready(), 7);
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/future-assert">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQberAKr9hIs-4bDA_CvcJiIgobI9A21j4XFcUbzFW8mziIAJ1hZIGDbWZ1dHVyZS1hc3NlcnRlMC4xLjBtZnV0dXJlX2Fzc2VydA
 [__link0]: https://docs.rs/future-assert/0.1.0/future_assert/trait.FutureTestExt.html
 [__link1]: https://docs.rs/future-assert/0.1.0/future_assert/fn.poll_once.html
 [__link2]: https://docs.rs/future-assert/0.1.0/future_assert/trait.FutureTestExt.html
