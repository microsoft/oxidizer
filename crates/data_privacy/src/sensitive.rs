// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use crate::{Classified, DataClass, RedactedDebug, RedactedDisplay, RedactionEngine};
use core::fmt::{Debug, Display, Formatter};
use data_privacy::IntoDataClass;

/// Size of the stack-allocated buffer used for formatting before falling back to heap allocation.
const STACK_BUFFER_SIZE: usize = 128;

/// A wrapper that dynamically classifies a value with a specific data class.
///
/// Use this wrapper in places where the data class of a value cannot be determined statically. When the data class is known
/// at compile time, prefer using specific classification types defined with the [`classified`](crate::classified) attribute macro.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd)]
pub struct Sensitive<T> {
    value: T,
    data_class: DataClass,
}

impl<T> Debug for Sensitive<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sensitive")
            .field("value", &"***")
            .field("data_class", &self.data_class)
            .finish()
    }
}

impl<T> Sensitive<T> {
    /// Creates a new instance of `Protected` with the given value and data class.
    pub fn new(value: T, data_class: impl IntoDataClass) -> Self {
        Self {
            value,
            data_class: data_class.into_data_class(),
        }
    }

    /// Changes the data class of this value.
    #[must_use]
    pub fn reclassify(self, data_class: impl IntoDataClass) -> Self {
        Self {
            data_class: data_class.into_data_class(),
            ..self
        }
    }

    /// Extracts the wrapped value, consuming the `Sensitive` wrapper.
    #[must_use]
    pub fn declassify_into(self) -> T {
        self.value
    }

    /// Returns a reference to the wrapped value.
    #[must_use]
    pub fn declassify_ref(&self) -> &T {
        &self.value
    }

    /// Returns a mutable reference to the wrapped value.
    #[must_use]
    pub fn declassify_mut(&mut self) -> &mut T {
        &mut self.value
    }
}

impl<T> Classified for Sensitive<T> {
    fn data_class(&self) -> &DataClass {
        &self.data_class
    }
}

impl<T> RedactedDebug for Sensitive<T>
where
    T: Debug,
{
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Converting from u64 to usize, value is known to be <= STACK_BUFFER_SIZE"
    )]
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        let v = &self.value;

        let mut local_buf = [0u8; STACK_BUFFER_SIZE];
        let amount = {
            let mut cursor = std::io::Cursor::new(&mut local_buf[..]);
            if std::io::Write::write_fmt(&mut cursor, format_args!("{v:?}")).is_ok() {
                cursor.position() as usize
            } else {
                local_buf.len() + 1 // force fallback case on write errors
            }
        };

        if amount <= local_buf.len() {
            // SAFETY: We know the buffer contains valid UTF-8 because the Debug impl can only write valid UTF-8.
            let s = unsafe { core::str::from_utf8_unchecked(&local_buf[..amount]) };

            engine.redact(self.data_class(), s, f)
        } else {
            // If the value is too large to fit in the buffer, we fall back to using the Debug format directly.
            engine.redact(self.data_class(), format!("{v:?}"), f)
        }
    }
}

impl<T> RedactedDisplay for Sensitive<T>
where
    T: Display,
{
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Converting from u64 to usize, value is known to be <= STACK_BUFFER_SIZE"
    )]
    fn fmt(&self, engine: &RedactionEngine, f: &mut Formatter) -> std::fmt::Result {
        let v = &self.value;

        let mut local_buf = [0u8; STACK_BUFFER_SIZE];
        let amount = {
            let mut cursor = std::io::Cursor::new(&mut local_buf[..]);
            if std::io::Write::write_fmt(&mut cursor, format_args!("{v}")).is_ok() {
                cursor.position() as usize
            } else {
                local_buf.len() + 1 // force fallback case on write errors
            }
        };

        if amount <= local_buf.len() {
            // SAFETY: We know the buffer contains valid UTF-8 because the Debug impl can only write valid UTF-8.
            let s = unsafe { core::str::from_utf8_unchecked(&local_buf[..amount]) };

            engine.redact(self.data_class(), s, f)
        } else {
            // If the value is too large to fit in the buffer, we fall back to using the Debug format directly.
            engine.redact(self.data_class(), format!("{v}"), f)
        }
    }
}
