// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

use crate::{Placement, TaskMeta};

/// Creates instances of [`TaskMeta`] to configure async tasks.
///
#[doc = include_str!("../../../doc/snippets/async_task.md")]
#[derive(Debug, Default)]
pub struct TaskMetaBuilder {
    name: Option<Cow<'static, str>>,
    placement: Placement,
}

impl TaskMetaBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn name(mut self, name: impl Into<Cow<'static, str>>) -> Self {
        self.name = Some(name.into());
        self
    }

    #[must_use]
    pub const fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    #[must_use]
    pub fn build(self) -> TaskMeta {
        TaskMeta {
            name: self.name,
            placement: self.placement,
        }
    }
}

impl From<TaskMetaBuilder> for TaskMeta {
    fn from(builder: TaskMetaBuilder) -> Self {
        builder.build()
    }
}

impl From<Placement> for TaskMeta {
    fn from(placement: Placement) -> Self {
        Self::with_placement(placement)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_meta_builder() {
        let meta = TaskMetaBuilder::new()
            .name("test")
            .placement(Placement::Any)
            .build();

        assert_eq!(meta.name(), Some("test"));
        assert_eq!(meta.placement(), Placement::Any);
    }

    #[test]
    fn without_anything_returns_default() {
        let meta = TaskMeta::builder().build();
        let default_meta = TaskMeta::default();

        assert_eq!(meta.name(), default_meta.name());
        assert_eq!(meta.placement(), default_meta.placement());
    }

    #[test]
    fn from_placement_ok() {
        let meta = TaskMeta::from(Placement::Background);
        assert_eq!(meta.placement(), Placement::Background);
    }

    #[test]
    fn from_builder_ok() {
        let meta: TaskMeta = TaskMetaBuilder::new()
            .placement(Placement::Background)
            .into();
        assert_eq!(meta.placement(), Placement::Background);
    }
}