// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::error::Error as StdError;
use std::fmt::{self, Display};

/// A failure to construct a runtime.
///
/// [`RuntimeBuilder::build`](crate::rt::config::RuntimeBuilder::build) returns this error
/// when the requested processor selection cannot be satisfied.
/// Construction failures deliberately have no public classification API.
#[derive(Debug)]
pub struct BuildError {
    requested: usize,
    available: usize,
}

impl Display for BuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "requested {} processors, but only {} are available",
            self.requested, self.available,
        )
    }
}

impl StdError for BuildError {}

impl BuildError {
    pub(crate) const fn insufficient_processors(requested: usize, available: usize) -> Self {
        Self { requested, available }
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::rt::Error as RuntimeError;

    assert_impl_all!(BuildError: StdError, Send, Sync);

    #[test]
    fn umbrella_error_preserves_construction_source() {
        let error = RuntimeError::from(BuildError::insufficient_processors(17, 3));
        assert!(error.source().unwrap().is::<BuildError>());
    }

    #[test]
    fn processor_failure_has_context_without_fabricated_source() {
        let error = BuildError::insufficient_processors(17, 3);
        let message = error.to_string();

        assert_eq!(
            (message.contains("17"), message.contains('3'), error.source().is_none(),),
            (true, true, true),
        );
    }
}
