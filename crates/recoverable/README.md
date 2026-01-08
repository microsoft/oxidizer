<div align="center">
 <img src="./logo.png" alt="Recoverable Logo" width="96">

# Recoverable

[![crate.io](https://img.shields.io/crates/v/recoverable.svg)](https://crates.io/crates/recoverable)
[![docs.rs](https://docs.rs/recoverable/badge.svg)](https://docs.rs/recoverable)
[![MSRV](https://img.shields.io/crates/msrv/recoverable)](https://crates.io/crates/recoverable)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/main.yml/badge.svg?event=push)](https://github.com/microsoft/oxidizer/actions/workflows/main.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](../../LICENSE)
<a href="../.."><img src="../../logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Recovery information and classification for resilience patterns.

## Why

This crate provides types for classifying conditions based on their **recoverability state**,
enabling consistent recovery behavior across different error types and resilience middleware.

## Recovery Information

The recovery information describes whether recovering from an operation might help, not whether
the operation succeeded or failed. Both successful operations and permanent failures
should use [`RecoveryInfo::never`][__link0] since recovery is not necessary or desirable.

## Core Types

* [`RecoveryInfo`][__link1]: Classifies conditions as recoverable (transient) or non-recoverable (permanent/successful).
* [`Recovery`][__link2]: A trait for types that can determine their recoverability.
* [`RecoveryKind`][__link3]: An enum representing the kind of recovery that can be attempted.

## Examples

### Recovery Error

```rust
use recoverable::{Recovery, RecoveryInfo, RecoveryKind};

#[derive(Debug)]
enum DatabaseError {
    ConnectionTimeout,
    InvalidCredentials,
    TableNotFound,
}

impl Recovery for DatabaseError {
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

### Retry Delay

You can specify when to retry an operation using the `delay` method:

```rust
use std::time::Duration;
use recoverable::{RecoveryInfo, RecoveryKind};

// Retry with a 30-second delay (e.g., from a Retry-After header)
let recovery = RecoveryInfo::retry().delay(Duration::from_secs(30));
assert_eq!(recovery.kind(), RecoveryKind::Retry);
assert_eq!(recovery.get_delay(), Some(Duration::from_secs(30)));

// Immediate retry
let immediate = RecoveryInfo::retry().delay(Duration::ZERO);
assert_eq!(immediate.get_delay(), Some(Duration::ZERO));
```


<hr/>
<sub>
This crate was developed as part of <a href="../..">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/recoverable">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGkYW0CYXSEGy4k8ldDFPOhG2VNeXtD5nnKG6EPY6OfW5wBG8g18NOFNdxpYXKEG4cFLMVQymhvG3_1rzbl1X55G-vZhEWC9_13GwjdQK0PrVchYWSBgmtyZWNvdmVyYWJsZWUwLjEuMA
 [__link0]: https://docs.rs/recoverable/0.1.0/recoverable/?search=RecoveryInfo::never
 [__link1]: https://docs.rs/recoverable/0.1.0/recoverable/struct.RecoveryInfo.html
 [__link2]: https://docs.rs/recoverable/0.1.0/recoverable/trait.Recovery.html
 [__link3]: https://docs.rs/recoverable/0.1.0/recoverable/enum.RecoveryKind.html
