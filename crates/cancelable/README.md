<div align="center">
 <img src="./logo.png" alt="Cancelable Logo" width="96">

# Cancelable

[![crate.io](https://img.shields.io/crates/v/cancelable.svg)](https://crates.io/crates/cancelable)
[![docs.rs](https://docs.rs/cancelable/badge.svg)](https://docs.rs/cancelable)
[![MSRV](https://img.shields.io/crates/msrv/cancelable)](https://crates.io/crates/cancelable)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Cooperative cancellation via tokens.

This module provides [`CancellationTokenSource`][__link0] and [`CancellationToken`][__link1],
modeled after the equivalent C# types. A source controls cancellation and
hands out lightweight, cloneable tokens for observers to check.

## Linked Sources

A linked source cancels when *any* of its parent tokens are canceled,
enabling composition of multiple cancellation signals:

```rust
use cancelable::CancellationTokenSource;

let first = CancellationTokenSource::new();
let second = CancellationTokenSource::new();

let linked = CancellationTokenSource::linked(&[first.token(), second.token()]);
let token = linked.token();

assert!(!token.is_cancelled());
second.cancel();
assert!(token.is_cancelled());
```

## Subscribers

Register callbacks that fire exactly once when cancellation occurs:

```rust
use cancelable::CancellationTokenSource;

let source = CancellationTokenSource::new();
source.subscribe(|| println!("Operation canceled"));
source.cancel();
```

## Futures

The [`CancelableExt`][__link2] trait adds a [`cancelable`][__link3] method
to any [`Future`][__link4], pairing it with a [`CancellationToken`][__link5] so that each
poll checks for cancellation before and after driving the inner future.

```rust
use cancelable::{CancelableExt, CancellationTokenSource};

let source = CancellationTokenSource::new();
let token = source.token();

let result = async { 42 }.cancelable(token).await?;
assert_eq!(result, 42);
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/cancelable">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjJhdIQbLiTyV0MU86EbZU15e0PmecoboQ9jo59bnAEbyDXw04U13GlhYvRhcoQbE3Iea_zSpkIbvcbCI0vEEEEb7KqsBtUtyHsbFhKo1iYbGRphZIGCamNhbmNlbGFibGVlMC4xLjA
 [__link0]: https://docs.rs/cancelable/0.1.0/cancelable/?search=CancellationTokenSource
 [__link1]: https://docs.rs/cancelable/0.1.0/cancelable/?search=CancellationToken
 [__link2]: https://docs.rs/cancelable/0.1.0/cancelable/?search=future::CancelableExt
 [__link3]: https://docs.rs/cancelable/0.1.0/cancelable/?search=future::CancelableExt::cancelable
 [__link4]: https://doc.rust-lang.org/stable/std/future/trait.Future.html
 [__link5]: https://docs.rs/cancelable/0.1.0/cancelable/?search=CancellationToken
