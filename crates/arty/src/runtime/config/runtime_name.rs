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
