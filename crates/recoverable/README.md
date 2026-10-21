<div align="center">
 <img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="Recoverable Logo" width="96">

# Recoverable

[![crate.io](https://img.shields.io/crates/v/recoverable.svg)](https://crates.io/crates/recoverable)
[![docs.rs](https://docs.rs/recoverable/badge.svg)](https://docs.rs/recoverable)
[![MSRV](https://img.shields.io/crates/msrv/recoverable)](https://crates.io/crates/recoverable)
[![CI](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml/badge.svg)](https://github.com/microsoft/oxidizer/actions/workflows/anvil-pr.yml)
[![Coverage](https://codecov.io/gh/microsoft/oxidizer/graph/badge.svg?token=FCUG0EL5TI)](https://codecov.io/gh/microsoft/oxidizer)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/microsoft/oxidizer/blob/main/LICENSE)
<a href="https://github.com/microsoft/oxidizer"><img src="https://raw.githubusercontent.com/microsoft/oxidizer/refs/heads/main/logo.svg" alt="This crate was developed as part of the Oxidizer project" width="20"></a>

</div>

Recovery classifications for resilient operations.

This crate classifies whether an operation can recover after a condition occurs.
Error types and resilience middleware can use the same classification.

Recovery information does not report whether an operation succeeded.
It reports whether a recovery attempt can change the outcome.
Use [`RecoveryInfo::never`][__link0] for successful operations and permanent failures.

## Core types

* [`RecoveryInfo`][__link1] contains a recovery kind and an optional delay.
* [`Recovery`][__link2] lets a type provide its recovery information.
* [`RecoveryKind`][__link3] identifies the available recovery action.

## Examples

### Classify errors

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
            // A retry can recover from this transient failure.
            DatabaseError::ConnectionTimeout => RecoveryInfo::retry(),
            // A retry cannot recover from these permanent failures.
            DatabaseError::InvalidCredentials => RecoveryInfo::never(),
            DatabaseError::TableNotFound => RecoveryInfo::never(),
        }
    }
}

let error = DatabaseError::ConnectionTimeout;
assert_eq!(error.recovery().kind(), RecoveryKind::Retry);

// Successful operations also use never() because they need no recovery.
let success_result: Result<(), DatabaseError> = Ok(());
```

### Set a retry delay

Use [`RecoveryInfo::delay`][__link4] to specify when to retry an operation.

```rust
use std::time::Duration;

use recoverable::{RecoveryInfo, RecoveryKind};

// Retry after 30 seconds, as specified by a Retry-After header.
let recovery = RecoveryInfo::retry().delay(Duration::from_secs(30));
assert_eq!(recovery.kind(), RecoveryKind::Retry);
assert_eq!(recovery.get_delay(), Some(Duration::from_secs(30)));

// Retry immediately.
let immediate = RecoveryInfo::retry().delay(Duration::ZERO);
assert_eq!(immediate.get_delay(), Some(Duration::ZERO));
```


<hr/>
<sub>
This crate was developed as part of <a href="https://github.com/microsoft/oxidizer">The Oxidizer Project</a>. Browse this crate's <a href="https://github.com/microsoft/oxidizer/tree/main/crates/recoverable">source code</a>.
</sub>

 [__cargo_doc2readme_dependencies_info]: ggGmYW0CYXZlMC43LjNhdIQborR2_k_xJd4bTcf2krrNPIcbP72Pw1UdRjkbim_eMDe2BBthYvRhcoQbr2p35Ft-e6Ab7SodP0ZOk-Eb88AyOJKgnPMbElRkLosz9oFhZIGCa3JlY292ZXJhYmxlZTAuMi4w
 [__link0]: https://docs.rs/recoverable/0.2.0/recoverable/?search=RecoveryInfo::never
 [__link1]: https://docs.rs/recoverable/0.2.0/recoverable/struct.RecoveryInfo.html
 [__link2]: https://docs.rs/recoverable/0.2.0/recoverable/trait.Recovery.html
 [__link3]: https://docs.rs/recoverable/0.2.0/recoverable/enum.RecoveryKind.html
 [__link4]: https://docs.rs/recoverable/0.2.0/recoverable/?search=RecoveryInfo::delay
