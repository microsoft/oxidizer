// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt::Debug;
use std::fmt::{Display, Formatter, Write};
use std::sync::Arc;

use crate::redaction_engine_builder::RedactionEngineBuilder;
use crate::redaction_engine_inner::RedactionEngineInner;
use crate::{DataClass, RedactedDebug, RedactedDisplay, RedactedToString};

/// Lets you apply redaction to classified data.
///
/// You use [`RedactionEngineBuilder`] to create an instance of this type.
/// The builder lets you configure exactly which redactor to use to redact individual data classes encountered
/// while producing telemetry.
///
/// ## Example
///
/// ```rust
/// use data_privacy::simple_redactor::{SimpleRedactor, SimpleRedactorMode};
/// use data_privacy::{RedactionEngine, classified, taxonomy};
///
/// // The taxonomy defines the different data classes we will use in our application.
/// #[taxonomy(simple)]
/// enum SimpleTaxonomy {
///     Sensitive,
///     ExtremelySensitive,
/// }
///
/// // A struct holding some data classified as sensitive.
/// #[classified(SimpleTaxonomy::Sensitive)]
/// struct Name(String);
///
/// struct Person {
///     name: Name, // a bit of sensitive data we should not leak in logs
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
/// let engine = RedactionEngine::builder()
///     .add_class_redactor(SimpleTaxonomy::Sensitive.data_class(), asterisk_redactor)
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
    inner: Arc<RedactionEngineInner>,
}

impl RedactionEngine {
    /// Constructs a new [`RedactionEngineBuilder`].
    #[must_use]
    pub fn builder() -> RedactionEngineBuilder {
        RedactionEngineBuilder::new()
    }

    #[must_use]
    pub(crate) fn new(mut inner: RedactionEngineInner) -> Self {
        inner.shrink();
        Self { inner: Arc::new(inner) }
    }

    /// Returns whether redaction would take place for the given data class.
    ///
    /// Returns `false` only when redaction has been explicitly suppressed for this data class
    /// via [`RedactionEngineBuilder::suppress_redaction`]. Returns `true` in all other cases,
    /// including when no specific redactor is registered (since the fallback redactor applies).
    #[must_use]
    pub fn would_redact(&self, data_class: &DataClass) -> bool {
        self.inner.would_redact(data_class)
    }

    /// Redacts a value implementing [`RedactedDebug`], sending the results to the output sink.
    ///
    /// # Errors
    ///
    /// This function returns [`Err`] if, and only if, writing to the provided output sink (which implements [`Write`]) returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`std::fmt::Result`] because writing to the underlying sink might fail and it must provide a way to propagate the fact that an error
    /// has occurred (as a [`std::fmt::Error`]) back up the stack.
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
    /// This function returns [`Err`] if, and only if, writing to the provided output sink (which implements [`Write`]) returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`std::fmt::Result`] because writing to the underlying sink might fail and it must provide a way to propagate the fact that an error
    /// has occurred (as a [`std::fmt::Error`]) back up the stack.
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
        value.to_redacted_string(self)
    }

    /// Redacts a string with an explicit data classification, sending the results to the output sink.
    ///
    /// # Errors
    ///
    /// This function returns [`Err`] if, and only if, writing to the provided output sink (which implements [`Write`]) returns [`Err`]. String redaction is considered an infallible operation;
    /// this function only returns a [`std::fmt::Result`] because writing to the underlying sink might fail and it must provide a way to propagate the fact that an error
    /// has occurred (as a [`std::fmt::Error`]) back up the stack.
    pub fn redact(&self, data_class: impl AsRef<DataClass>, value: impl AsRef<str>, output: &mut impl Write) -> core::fmt::Result {
        let data_class_ref = data_class.as_ref();
        let value_str = value.as_ref();

        if let Some(redactor) = self.inner.resolve(data_class_ref) {
            redactor.redact(data_class_ref, value_str, output)
        } else {
            // Redaction has been explicitly suppressed for this data class; pass through unmodified,
            // bypassing both class-specific and fallback redactors.
            output.write_str(value_str)
        }
    }
}

impl Debug for RedactionEngine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.inner.fmt(f)
    }
}
