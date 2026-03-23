// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::redaction_engine_inner::RedactionEngineInner;
use crate::{IntoDataClass, RedactionEngine, Redactor};

/// A builder for creating a [`RedactionEngine`].
#[derive(Debug)]
pub struct RedactionEngineBuilder {
    inner: RedactionEngineInner,
}

impl RedactionEngineBuilder {
    /// Creates a new instance of `RedactionEngineBuilder`.
    ///
    /// This is initialized with no registered redactors and a fallback redactor that erases the input.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            inner: RedactionEngineInner::default(),
        }
    }

    /// Adds a redactor for a specific data class.
    ///
    /// Whenever the redaction engine encounters data of this class, it will use the provided redactor.
    ///
    /// If the same data class was previously suppressed or assigned a different redactor,
    /// this call overrides that configuration (last call wins).
    #[must_use]
    pub fn add_class_redactor(mut self, data_class: impl IntoDataClass, redactor: impl Redactor + Send + Sync + 'static) -> Self {
        self.inner.insert(data_class, redactor);
        self
    }

    /// Adds a redactor that's a fallback for when there is no redactor registered for a particular
    /// data class.
    ///
    /// By default, the fallback uses a [`SimpleRedactor`](crate::simple_redactor::SimpleRedactor) configured with
    /// [`SimpleRedactorMode::Erase`](crate::simple_redactor::SimpleRedactorMode::Erase), which simply erases the original string.
    #[must_use]
    pub fn set_fallback_redactor(mut self, redactor: impl Redactor + Send + Sync + 'static) -> Self {
        self.inner.set_fallback(redactor);
        self
    }

    /// Suppresses redaction for a specific data class.
    ///
    /// Data of this class will pass through unmodified, bypassing both class-specific and fallback redactors.
    ///
    /// If the same data class was previously assigned a redactor, this call overrides
    /// that configuration (last call wins).
    #[must_use]
    pub fn suppress_redaction(mut self, data_class: impl IntoDataClass) -> Self {
        self.inner.suppress(data_class);
        self
    }

    /// Builds the `RedactionEngine`.
    #[must_use]
    pub fn build(self) -> RedactionEngine {
        RedactionEngine::new(self.inner)
    }
}
