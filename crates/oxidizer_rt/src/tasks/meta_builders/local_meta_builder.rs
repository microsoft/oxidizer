// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::borrow::Cow;

use crate::LocalTaskMeta;

/// Creates instances of [`LocalTaskMeta`] to configure local async tasks.
///
#[doc = include_str!("../../../doc/snippets/async_task.md")]
///
#[doc = include_str!("../../../doc/snippets/local_task.md")]
#[derive(Debug, Default)]
pub struct LocalTaskMetaBuilder {
    name: Option<Cow<'static, str>>,
}

impl LocalTaskMetaBuilder {
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
    pub fn build(self) -> LocalTaskMeta {
        LocalTaskMeta { name: self.name }
    }
}

impl From<LocalTaskMetaBuilder> for LocalTaskMeta {
    fn from(builder: LocalTaskMetaBuilder) -> Self {
        builder.build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_task_meta_builder() {
        let meta = LocalTaskMetaBuilder::new().name("test").build();

        assert_eq!(meta.name(), Some("test"));
    }

    #[test]
    fn without_anything_returns_default() {
        let meta = LocalTaskMeta::builder().build();
        let default_meta = LocalTaskMeta::default();

        assert_eq!(meta.name(), default_meta.name());
    }

    #[test]
    fn from_builder_ok() {
        let meta: LocalTaskMeta = LocalTaskMetaBuilder::new().name("dummy").into();
        assert_eq!(meta.name(), Some("dummy"));
    }
}