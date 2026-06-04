// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt::{Debug, Display, Formatter};

use data_privacy_core::{Classified, DataClass, IntoDataClass, RedactedDebug, RedactedDisplay, Redactor};

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
    /// Creates a new instance of [`Sensitive`] with the given value and data class.
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

    /// Extracts the wrapped value, consuming the [`Sensitive`] wrapper.
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
    fn fmt(&self, redactor: &dyn Redactor, f: &mut Formatter) -> std::fmt::Result {
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

            redactor.redact(self.data_class(), s, f)
        } else {
            // If the value is too large to fit in the buffer, we fall back to using the Debug format directly.
            redactor.redact(self.data_class(), &format!("{v:?}"), f)
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
    fn fmt(&self, redactor: &dyn Redactor, f: &mut Formatter) -> std::fmt::Result {
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
            // SAFETY: We know the buffer contains valid UTF-8 because the Display impl can only write valid UTF-8.
            let s = unsafe { core::str::from_utf8_unchecked(&local_buf[..amount]) };

            redactor.redact(self.data_class(), s, f)
        } else {
            // If the value is too large to fit in the buffer, we fall back to using the Display format directly.
            redactor.redact(self.data_class(), &format!("{v}"), f)
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use core::fmt::Write;

    use insta::assert_snapshot;

    use crate::{DataClass, RedactedDebug, RedactedDisplay, RedactedToString, Redactor, Sensitive};

    /// A test redactor that wraps values in brackets with the data class.
    struct TagRedactor;

    impl Redactor for TagRedactor {
        fn redacts(&self, _data_class: &DataClass) -> bool {
            true
        }

        fn redact(&self, data_class: &DataClass, value: &str, output: &mut dyn Write) -> core::fmt::Result {
            write!(output, "<{}/{}:{}>", data_class.taxonomy(), data_class.name(), value)
        }
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn redacted_debug_produces_nonempty_output() {
        struct Adapter<'a, T: ?Sized> {
            inner: &'a T,
            redactor: &'a dyn Redactor,
        }
        impl<T: RedactedDebug + ?Sized> std::fmt::Display for Adapter<'_, T> {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                <T as RedactedDebug>::fmt(self.inner, self.redactor, f)
            }
        }
        let sensitive = Sensitive::new("secret", DataClass::new("test", "pii"));
        let result = Adapter {
            inner: &sensitive,
            redactor: &TagRedactor as &dyn Redactor,
        }
        .to_string();
        assert_snapshot!(result, @r#"<test/pii:"secret">"#);
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn redacted_display_produces_nonempty_output() {
        struct Adapter<'a, T: ?Sized> {
            inner: &'a T,
            redactor: &'a dyn Redactor,
        }
        impl<T: RedactedDisplay + ?Sized> std::fmt::Display for Adapter<'_, T> {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                <T as RedactedDisplay>::fmt(self.inner, self.redactor, f)
            }
        }
        let sensitive = Sensitive::new("secret", DataClass::new("test", "pii"));
        let result = Adapter {
            inner: &sensitive,
            redactor: &TagRedactor as &dyn Redactor,
        }
        .to_string();
        assert_snapshot!(result, @"<test/pii:secret>");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn redacted_to_string_produces_nonempty_output() {
        let sensitive = Sensitive::new("secret", DataClass::new("test", "pii"));
        let result = sensitive.to_redacted_string(&TagRedactor);
        assert_snapshot!(result, @"<test/pii:secret>");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn redacted_display_at_exact_buffer_boundary() {
        // STACK_BUFFER_SIZE is 128, so a Display output of exactly 128 bytes tests the `<=` boundary
        let value = "x".repeat(128);
        let sensitive = Sensitive::new(value, DataClass::new("test", "pii"));
        let result = sensitive.to_redacted_string(&TagRedactor);
        assert_snapshot!(result, @"<test/pii:xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx>");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn redacted_debug_at_exact_buffer_boundary() {
        // Debug of a String adds quotes: "xxx" -> 126 chars + 2 quotes = 128 bytes
        struct Adapter<'a, T: ?Sized> {
            inner: &'a T,
            redactor: &'a dyn Redactor,
        }
        impl<T: RedactedDebug + ?Sized> std::fmt::Display for Adapter<'_, T> {
            fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                <T as RedactedDebug>::fmt(self.inner, self.redactor, f)
            }
        }
        let value = "x".repeat(126);
        let sensitive = Sensitive::new(value, DataClass::new("test", "pii"));
        let result = Adapter {
            inner: &sensitive,
            redactor: &TagRedactor as &dyn Redactor,
        }
        .to_string();
        assert_snapshot!(result, @r#"<test/pii:"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx">"#);
    }

    #[test]
    fn reclassify_changes_data_class() {
        use crate::Classified;

        let sensitive = Sensitive::new("secret", DataClass::new("original", "pii"));
        let reclassified = sensitive.reclassify(DataClass::new("updated", "eupi"));
        assert_eq!(reclassified.data_class().taxonomy(), "updated");
        assert_eq!(reclassified.data_class().name(), "eupi");
    }
}
