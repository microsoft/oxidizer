// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

use crate::Placement;
use crate::meta_builders::TaskMetaBuilder;

/// Metadata describing an async task, used to control how the task is scheduled and executed.
///
#[doc = include_str!("../../doc/snippets/async_task.md")]
#[derive(Clone, Debug, Default)]
pub struct TaskMeta {
    pub(super) name: Option<Cow<'static, str>>,
    pub(super) placement: Placement,
}

impl TaskMeta {
    #[must_use]
    pub const fn with_placement(placement: Placement) -> Self {
        Self {
            name: None,
            placement,
        }
    }

    #[cfg_attr(test, mutants::skip)] // Just a default-returning function, mutation can be no-op.
    #[must_use]
    pub fn builder() -> TaskMetaBuilder {
        TaskMetaBuilder::new()
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_ref().map(AsRef::as_ref)
    }

    #[cfg_attr(test, mutants::skip)] // There is only 1 value in the enum, mutations are no-op.
    #[must_use]
    pub const fn placement(&self) -> Placement {
        self.placement
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_placement_ok() {
        let meta = TaskMeta::with_placement(Placement::CurrentRegion);

        assert_eq!(meta.name(), None);
        assert_eq!(meta.placement(), Placement::CurrentRegion);
    }
}