// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::redactors::Redactors;
use crate::{DataClass, RedactedDebug, RedactedDisplay, RedactedToString, Redactor};
use core::fmt::Debug;
use data_privacy::IntoDataClass;
use std::fmt::{Display, Formatter, Write};
use std::sync::Arc;

/// Lets you apply redaction to classified data.
///
/// You use [`RedactionEngineBuilder`](crate::RedactionEngineBuilder) to create an instance of this type.
/// The builder lets you configure exactly which redactor to use to redact individual data classes encountered
/// while producing telemetry.
///
/// ## Example
///
/// ```rust
/// use core::fmt::Write;
/// use data_privacy::{classified, RedactionEngineBuilder, Redactor, SimpleRedactor, SimpleRedactorMode, taxonomy};
///
/// // The taxonomy defines the different data classes we will use in our application.
/// #[taxonomy(simple)]
/// enum SimpleTaxonomy {
///    Sensitive,
///    ExtremelySensitive,
/// }
///
/// // A struct holding some data classified as sensitive.
/// #[classified(SimpleTaxonomy::Sensitive)]
/// struct Name(String);
///
/// struct Person {
///     name: Name,     // a bit of sensitive data we should not leak in logs
///     age: u32,
/// }
///
/// let person = Person {
///     name: Name("John Doe".to_string()),
///     age: 30,
/// };
///
/// let asterisk_redactor = SimpleRedactor::new();
/// let erasing_redactor = SimpleRedactor::with_mode(SimpleRedactorMode::Erase);
///
/// // Create the redaction engine. This is typically done once when the application starts.
/// let engine = RedactionEngineBuilder::new()
///     .add_class_redactor(&SimpleTaxonomy::Sensitive.data_class(), asterisk_redactor)
///     .set_fallback_redactor(erasing_redactor)
///     .build();
///
/// let mut output_buffer = String::new();
/// _ = engine.redacted_display(&person.name, &mut output_buffer);
///
/// // check that the data in the output buffer has indeed been redacted as expected.
/// assert_eq!(output_buffer, "********");
/// ```
#[derive(Clone, Default)]
pub struct RedactionEngine {
    redactors: Arc<Redactors>,
}

impl RedactionEngine {
    /// Constructs a new [`RedactionEngineBuilder`].
    #[must_use]
    pub fn builder() -> RedactionEngineBuilder {
        RedactionEngineBuilder::new()
    }

    #[must_use]
    pub fn new(mut redactors: Redactors) -> Self {
        redactors.shrink();
        Self {
            redactors: Arc::new(redactors),
        }
    }

    /// Redacts a value implementing [`RedactedDebug`], sending the results to the output sink.
    ///
    /// # Errors
    ///
    /// This function should return [`Err`] if, and only if, the provided [`Formatter`] returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`std::fmt::Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    pub fn redacted_debug(&self, value: &impl RedactedDebug, output: &mut impl Write) -> core::fmt::Result {
        struct DebugFormatter<'a, RD>
        where
            RD: RedactedDebug + ?Sized,
        {
            engine: &'a RedactionEngine,
            value: &'a RD,
        }

        impl<RD> Debug for DebugFormatter<'_, RD>
        where
            RD: RedactedDebug + ?Sized,
        {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                self.value.fmt(self.engine, f)
            }
        }

        let d = DebugFormatter { engine: self, value };
        write!(output, "{d:?}")
    }

    /// Redacts a value implementing [`RedactedDisplay`], sending the results to the output sink.
    ///
    /// # Errors
    ///
    /// This function should return [`Err`] if, and only if, the provided [`Formatter`] returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`std::fmt::Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    pub fn redacted_display(&self, value: &impl RedactedDisplay, output: &mut impl Write) -> core::fmt::Result {
        struct DisplayFormatter<'a, RD>
        where
            RD: RedactedDisplay + ?Sized,
        {
            engine: &'a RedactionEngine,
            value: &'a RD,
        }

        impl<RD> Display for DisplayFormatter<'_, RD>
        where
            RD: RedactedDisplay + ?Sized,
        {
            fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
                self.value.fmt(self.engine, f)
            }
        }

        let d = DisplayFormatter { engine: self, value };
        write!(output, "{d}")
    }

    /// Redacts a value implementing [`RedactedToString`] and returns the redacted string.
    pub fn redacted_to_string(&self, value: &impl RedactedToString) -> String {
        value.to_string(self)
    }

    /// Redacts a string with an explicit data classification, sending the results to the output sink.
    ///
    /// # Errors
    ///
    /// This function should return [`Err`] if, and only if, the provided [`Formatter`] returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`std::fmt::Result`] because writing to the underlying stream might fail and it must provide a way to propagate the fact that an error
    /// has occurred back up the stack.
    pub fn redact(&self, data_class: impl AsRef<DataClass>, value: impl AsRef<str>, output: &mut impl Write) -> core::fmt::Result {
        let redactor = self.redactors.get_or_fallback(data_class.as_ref());
        redactor.redact(data_class.as_ref(), value.as_ref(), output)
    }
}

impl Debug for RedactionEngine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.redactors.fmt(f)
    }
}

/// A builder for creating a [`RedactionEngine`].
pub struct RedactionEngineBuilder {
    redactors: Redactors,
}

impl RedactionEngineBuilder {
    /// Creates a new instance of `RedactionEngineBuilder`.
    ///
    /// This is initialized with no registered redactors and a fallback redactor that erases the input.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            redactors: Redactors::default(),
        }
    }

    /// Adds a redactor for a specific data class.
    ///
    /// Whenever the redaction engine encounters data of this class, it will use the provided redactor.
    #[must_use]
    pub fn add_class_redactor(mut self, data_class: impl IntoDataClass, redactor: impl Redactor + Send + Sync + 'static) -> Self {
        self.redactors.insert(data_class.into_data_class(), redactor);
        self
    }

    /// Adds a redactor that's a fallback for when there is no redactor registered for a particular
    /// data class.
    ///
    /// The default fallback is to use an `ErasingRedactor`, which simply erases the original string.
    #[must_use]
    pub fn set_fallback_redactor(mut self, redactor: impl Redactor + Send + Sync + 'static) -> Self {
        self.redactors.set_fallback(redactor);
        self
    }

    /// Builds the `RedactionEngine`.
    #[must_use]
    pub fn build(self) -> RedactionEngine {
        RedactionEngine::new(self.redactors)
    }
}

impl Default for RedactionEngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl Debug for RedactionEngineBuilder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.redactors.fmt(f)
    }
}
