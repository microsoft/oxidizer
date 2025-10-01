<div style="text-align: center">
 <img src="./logo.png" alt="Recoverable Logo" width="128">

# Recoverable

[![crate.io](https://img.shields.io/crates/v/recoverable.svg)](https://crates.io/crates/recoverable)
[![docs.rs](https://docs.rs/recoverable/badge.svg)](https://docs.rs/recoverable)
[![CI](https://github.com/microsoft/oxidizer/workflows/main/badge.svg)](https://github.com/microsoft/oxidizer/actions)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../LICENSE)

</div>

- [Recoverable](#recoverable)
  - [Summary](#summary)

## Summary

<!-- cargo-rdme start -->

Recovery metadata and classification for resilience patterns.

This crate provides types for classifying error conditions as recoverable or non-recoverable,
enabling consistent retry behavior across different error types and resilience middleware.

## Core Types

- [`Recovery`]: Classifies errors as recoverable (transient) or non-recoverable (permanent).
- [`Recover`]: A trait for types that can determine their recoverability.
- [`RecoveryKind`]: An enum representing the kind of recovery that can be attempted.

## Examples

```rust
use recoverable::{Recover, Recovery, RecoveryKind};

#[derive(Debug)]
enum DatabaseError {
    ConnectionTimeout,
    InvalidCredentials,
    TableNotFound,
}

impl Recover for DatabaseError {
    fn recovery(&self) -> Recovery {
        match self {
            DatabaseError::ConnectionTimeout => Recovery::retry(),
            DatabaseError::InvalidCredentials => Recovery::never(),
            DatabaseError::TableNotFound => Recovery::never(),
        }
    }
}

let error = DatabaseError::ConnectionTimeout;
assert_eq!(error.recovery().kind(), RecoveryKind::Retry);
```

<!-- cargo-rdme end -->

<div style="font-size: 75%" ><hr/>

This crate was developed as part of [The Oxidizer Project](https://github.com/microsoft/oxidizer).

</div>
