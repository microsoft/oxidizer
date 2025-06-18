// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

use crate::{SystemTaskCategory, SystemTaskMeta};

/// Creates instances of [`SystemTaskMeta`] to configure system tasks.
///
#[doc = include_str!("../../../doc/snippets/system_task.md")]
#[derive(Debug, Default)]
pub struct SystemTaskMetaBuilder {
    name: Option<Cow<'static, str>>,
    category: SystemTaskCategory,
}

impl SystemTaskMetaBuilder {
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
    pub const fn category(mut self, category: SystemTaskCategory) -> Self {
        self.category = category;
        self
    }

    #[must_use]
    pub fn build(self) -> SystemTaskMeta {
        SystemTaskMeta {
            name: self.name,
            category: self.category,
        }
    }
}

impl From<SystemTaskMetaBuilder> for SystemTaskMeta {
    fn from(builder: SystemTaskMetaBuilder) -> Self {
        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_task_meta_builder() {
        let meta = SystemTaskMetaBuilder::new()
            .name("test")
            .category(SystemTaskCategory::ReleaseResources)
            .build();

        assert_eq!(meta.name(), Some("test"));
        assert_eq!(meta.category(), SystemTaskCategory::ReleaseResources);
    }

    #[test]
    fn without_anything_returns_default() {
        let meta = SystemTaskMeta::builder().build();
        let default_meta = SystemTaskMeta::default();

        assert_eq!(meta.name(), default_meta.name());
        assert_eq!(meta.category(), default_meta.category());
    }

    #[test]
    fn from_builder_ok() {
        let meta: SystemTaskMeta = SystemTaskMetaBuilder::new().name("dummy").into();
        assert_eq!(meta.name(), Some("dummy"));
    }
}