// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::sync::Arc;

use crate::runtime::error::Error;

/// Creates thread names for one runtime.
#[derive(Clone, Debug)]
pub(crate) struct RuntimeName(Arc<str>);

impl RuntimeName {
    pub(crate) fn new(name: impl AsRef<str>) -> Self {
        Self(Arc::from(name.as_ref()))
    }

    pub(crate) fn async_worker(&self, index: usize) -> String {
        format!("{}-async-{index}", self.0)
    }

    pub(crate) fn blocking_worker(&self, async_worker_index: Option<usize>, thread_index: usize) -> String {
        async_worker_index.map_or_else(
            || format!("{}-blocking-{thread_index}", self.0),
            |worker_index| format!("{}-blocking-{worker_index}-{thread_index}", self.0),
        )
    }

    pub(crate) fn validate(&self) -> Result<(), Error> {
        if self.0.contains('\0') {
            return Err(Error::invalid_name());
        }
        Ok(())
    }
}

impl Default for RuntimeName {
    fn default() -> Self {
        Self::new("arty")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))] // Test scaffolding is not runtime behavior.
mod tests {
    use super::*;

    #[test]
    fn default_uses_arty() {
        let name = RuntimeName::default();

        assert_eq!(name.async_worker(2), "arty-async-2");
        assert_eq!(name.blocking_worker(None, 3), "arty-blocking-3");
        assert_eq!(name.blocking_worker(Some(2), 3), "arty-blocking-2-3");
    }

    #[test]
    fn custom_name_is_copied() {
        let source = String::from("requests");
        let name = RuntimeName::new(&source);
        drop(source);

        assert_eq!(name.async_worker(2), "requests-async-2");
        assert_eq!(name.blocking_worker(None, 3), "requests-blocking-3");
        assert_eq!(name.blocking_worker(Some(2), 3), "requests-blocking-2-3");
    }

    #[test]
    fn validation_checks_the_base_name() {
        RuntimeName::new("requests").validate().unwrap();
        assert_eq!(
            RuntimeName::new("invalid\0name").validate().unwrap_err().to_string(),
            "runtime name must not contain a null character"
        );
    }
}
