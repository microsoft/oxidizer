// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

use crate::meta_builders::LocalTaskMetaBuilder;

/// Metadata describing a local async task,
/// used to control how the task is scheduled and executed.
///
#[doc = include_str!("../../doc/snippets/async_task.md")]
///
#[doc = include_str!("../../doc/snippets/local_task.md")]
#[derive(Clone, Debug, Default)]
pub struct LocalTaskMeta {
    pub(super) name: Option<Cow<'static, str>>,
}

impl LocalTaskMeta {
    #[cfg_attr(test, mutants::skip)] // Just a default-returning function, mutation can be no-op.
    #[must_use]
    pub fn builder() -> LocalTaskMetaBuilder {
        LocalTaskMetaBuilder::new()
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_ref().map(AsRef::as_ref)
    }
}