// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

use crate::SystemTaskCategory;
use crate::meta_builders::SystemTaskMetaBuilder;

/// Metadata describing a system task, used to control how the task is scheduled and executed.
///
#[doc = include_str!("../../doc/snippets/system_task.md")]
#[derive(Clone, Debug, Default)]
pub struct SystemTaskMeta {
    pub(super) name: Option<Cow<'static, str>>,
    pub(super) category: SystemTaskCategory,
}

impl SystemTaskMeta {
    #[cfg_attr(test, mutants::skip)] // Just a default-returning function, mutation can be no-op.
    #[must_use]
    pub fn builder() -> SystemTaskMetaBuilder {
        SystemTaskMetaBuilder::new()
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_ref().map(AsRef::as_ref)
    }

    #[must_use]
    pub const fn category(&self) -> SystemTaskCategory {
        self.category
    }
}