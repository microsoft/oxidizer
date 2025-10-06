<div align="center">
 <img src="./logo.png" alt="Recoverable Logo" width="128">

# Recoverable

[![crate.io](https://img.shields.io/crates/v/recoverable.svg)](https://crates.io/crates/recoverable)
[![docs.rs](https://docs.rs/recoverable/badge.svg)](https://docs.rs/recoverable)
[![CI](https://github.com/microsoft/oxidizer/workflows/main/badge.svg)](https://github.com/microsoft/oxidizer/actions)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

</div>

- [Summary](#summary)
- [Core Types](#core-types)
- [Examples](#examples)

## Summary

<!-- cargo-rdme start -->

Recovery information and classification for resilience patterns.

This crate provides types for classifying conditions based on their **recoverability state**,
enabling consistent recovery behavior across different error types and resilience middleware.

The recovery information describes whether recovering from an operation might help, not whether
the operation succeeded or failed. Both successful operations and permanent failures
should use [`RecoveryInfo::never`](https://docs.rs/recoverable/latest/recoverable/struct.RecoveryInfo.html#method.never) since recovery won't change the outcome.

## Core Types

- [`RecoveryInfo`](https://docs.rs/recoverable/latest/recoverable/struct.RecoveryInfo.html): Classifies conditions as recoverable (transient) or non-recoverable (permanent/successful).
- [`Recoverable`](https://docs.rs/recoverable/latest/recoverable/trait.Recoverable.html): A trait for types that can determine their recoverability.
- [`RecoveryKind`](https://docs.rs/recoverable/latest/recoverable/enum.RecoveryKind.html): An enum representing the kind of recovery that can be attempted.

## Examples

```rust
use recoverable::{Recoverable, RecoveryInfo, RecoveryKind};

#[derive(Debug)]
enum DatabaseError {
    ConnectionTimeout,
    InvalidCredentials,
    TableNotFound,
}

impl Recoverable for DatabaseError {
    fn recovery(&self) -> RecoveryInfo {
        match self {
            // Transient failure - might succeed if retried
            DatabaseError::ConnectionTimeout => RecoveryInfo::retry(),
            // Permanent failures - retrying won't help
            DatabaseError::InvalidCredentials => RecoveryInfo::never(),
            DatabaseError::TableNotFound => RecoveryInfo::never(),
        }
    }
}

let error = DatabaseError::ConnectionTimeout;
assert_eq!(error.recovery().kind(), RecoveryKind::Retry);

// For successful operations, also use never() since retry is unnecessary
let success_result: Result<(), DatabaseError> = Ok(());
// If we had a wrapper type for success, it would also return RecoveryInfo::never()
```

<!-- cargo-rdme end -->

<div style="font-size: 75%" ><hr/>

This crate was developed as part of [The Oxidizer Project](https://github.com/microsoft/oxidizer).

</div>
