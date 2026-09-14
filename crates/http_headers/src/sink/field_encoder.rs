// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Encoding a typed field into the field lines a sink stores.

use std::marker::PhantomData;

use crate::sink::{EncodedValues, InsertError};
use crate::{FieldValue, FieldValueRef};

/// Describes whether an encoded field value contains sensitive data.
///
/// # Examples
///
/// ```rust
/// use http_headers::FieldSensitivity;
///
/// assert!(!FieldSensitivity::NonSensitive.is_sensitive());
/// assert!(FieldSensitivity::Sensitive.is_sensitive());
/// ```
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FieldSensitivity {
    /// The value may be logged or displayed normally.
    NonSensitive,
    /// The value should be redacted by supporting containers and formatters.
    Sensitive,
}

impl FieldSensitivity {
    pub(crate) const fn from_sensitive(sensitive: bool) -> Self {
        if sensitive { Self::Sensitive } else { Self::NonSensitive }
    }

    /// Returns whether the value is sensitive.
    #[must_use]
    pub const fn is_sensitive(self) -> bool {
        matches!(self, Self::Sensitive)
    }
}

/// Writes one encoded HTTP field value into a sink-provided destination.
///
/// The [`FieldEncodeOutput`] example shows a writer implementation that
/// validates the announced length and preserves sensitivity.
pub trait FieldValueWriter {
    /// Appends bytes to the current field value.
    ///
    /// # Errors
    ///
    /// Returns an error when the destination cannot accept the bytes.
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), InsertError>;

    /// Completes this field value.
    ///
    /// # Errors
    ///
    /// Returns an error when the completed bytes are not a valid field value
    /// or the destination cannot retain them.
    fn finish(self) -> Result<(), InsertError>;
}

/// Receives the field values emitted by a [`FieldEncoder`], one per field line.
///
/// Implement this trait when a [`super::FieldSink`] needs to write encoded values
/// directly into its own container type.
///
/// # Examples
///
/// ```rust
/// use http_headers::sink::{
///     FieldEncodeOutput, FieldEncoder, FieldValueWriter, InsertError, InsertErrorKind,
///     U64Encoder, ValueRefsEncoder,
/// };
/// use http_headers::{FieldSensitivity, FieldValue, FieldValueRef};
///
/// #[derive(Default)]
/// struct Output(Vec<FieldValue>);
///
/// struct Writer<'a> {
///     output: &'a mut Vec<FieldValue>,
///     bytes: Vec<u8>,
///     expected: usize,
///     sensitivity: FieldSensitivity,
/// }
///
/// impl FieldValueWriter for Writer<'_> {
///     fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), InsertError> {
///         if self.bytes.len().saturating_add(bytes.len()) > self.expected {
///             return Err(InsertError::new(InsertErrorKind::InvalidEncoding));
///         }
///         self.bytes.extend_from_slice(bytes);
///         Ok(())
///     }
///
///     fn finish(self) -> Result<(), InsertError> {
///         if self.bytes.len() != self.expected {
///             return Err(InsertError::new(InsertErrorKind::InvalidEncoding));
///         }
///         let value = FieldValue::from_bytes(self.bytes)
///             .map_err(|_| InsertError::new(InsertErrorKind::InvalidValue))?
///             .with_sensitivity(self.sensitivity);
///         self.output.push(value);
///         Ok(())
///     }
/// }
///
/// impl FieldEncodeOutput for Output {
///     type Writer<'a> = Writer<'a>;
///
///     fn begin_value(
///         &mut self,
///         length: usize,
///         sensitivity: FieldSensitivity,
///     ) -> Result<Self::Writer<'_>, InsertError> {
///         let mut bytes = Vec::new();
///         bytes
///             .try_reserve_exact(length)
///             .map_err(|_| InsertError::new(InsertErrorKind::AllocationFailed))?;
///         Ok(Writer {
///             output: &mut self.0,
///             bytes,
///             expected: length,
///             sensitivity,
///         })
///     }
///
///     fn push_value(&mut self, value: FieldValue) -> Result<(), InsertError> {
///         self.0.push(value);
///         Ok(())
///     }
/// }
///
/// let mut output = Output::default();
/// U64Encoder::new(42).encode(&mut output)?;
/// ValueRefsEncoder::new([FieldValueRef::new(b"a=1"), FieldValueRef::new(b"b=2")])
///     .with_sensitivity(FieldSensitivity::Sensitive)
///     .encode(&mut output)?;
///
/// assert_eq!(output.0[0].as_bytes(), b"42");
/// assert!(output.0[1..].iter().all(FieldValue::is_sensitive));
/// # Ok::<(), InsertError>(())
/// ```
pub trait FieldEncodeOutput {
    /// Writer used for one newly encoded field value.
    type Writer<'a>: FieldValueWriter
    where
        Self: 'a;

    /// Starts one field value with its exact encoded length and sensitivity.
    ///
    /// The encoder must write exactly `length` bytes before calling
    /// [`FieldValueWriter::finish`]. Implementations must reject a completed
    /// value with a different length and preserve the sensitivity marker.
    ///
    /// # Errors
    ///
    /// Returns an error when the destination cannot reserve the requested
    /// storage.
    fn begin_value(&mut self, length: usize, sensitivity: FieldSensitivity) -> Result<Self::Writer<'_>, InsertError>;

    /// Receives an already-owned field value.
    ///
    /// # Errors
    ///
    /// Returns an error when the destination cannot retain the value.
    fn push_value(&mut self, value: FieldValue) -> Result<(), InsertError>;

    /// Writes one unsigned decimal field value.
    ///
    /// # Errors
    ///
    /// Returns an error when the destination cannot retain the value.
    fn push_u64(&mut self, mut value: u64) -> Result<(), InsertError> {
        let mut storage = [0_u8; 20];
        let mut start = storage.len();
        loop {
            start -= 1;
            storage[start] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        let digits = &storage[start..];
        let mut writer = self.begin_value(digits.len(), FieldSensitivity::NonSensitive)?;
        writer.write_bytes(digits)?;
        writer.finish()
    }
}

/// A value that can encode one field's lines.
///
/// The [`FieldEncodeOutput`] example implements this trait's destination and
/// drives it with reusable encoders.
pub trait FieldEncoder {
    /// Emits every field value into `output`, one per field line.
    ///
    /// # Errors
    ///
    /// Returns an error when the destination cannot encode or retain the
    /// complete field.
    fn encode<O>(self, output: &mut O) -> Result<(), InsertError>
    where
        O: FieldEncodeOutput;
}

impl FieldEncoder for FieldValueRef<'_> {
    fn encode<O>(self, output: &mut O) -> Result<(), InsertError>
    where
        O: FieldEncodeOutput,
    {
        let mut writer = output.begin_value(self.as_bytes().len(), FieldSensitivity::from_sensitive(self.is_sensitive()))?;
        writer.write_bytes(self.as_bytes())?;
        writer.finish()
    }
}

/// Encodes borrowed values in iteration order, one field line each.
///
/// The [`FieldEncodeOutput`] example encodes multiple borrowed values and
/// applies one sensitivity marker to every emitted line.
#[derive(Clone, Debug)]
pub struct ValueRefsEncoder<'a, I> {
    values: I,
    sensitivity: Option<FieldSensitivity>,
    marker: PhantomData<FieldValueRef<'a>>,
}

impl<I> ValueRefsEncoder<'_, I> {
    /// Creates an encoder that preserves each borrowed value's sensitivity.
    #[must_use]
    pub const fn new(values: I) -> Self {
        Self {
            values,
            sensitivity: None,
            marker: PhantomData,
        }
    }

    /// Sets the sensitivity marker on every emitted field line.
    ///
    /// This replaces the marker carried by each individual
    /// [`FieldValueRef`].
    #[must_use]
    pub const fn with_sensitivity(mut self, sensitivity: FieldSensitivity) -> Self {
        self.sensitivity = Some(sensitivity);
        self
    }

    #[cfg(any(test, feature = "headers-set-cookie",))]
    pub(crate) const fn with_sensitive(mut self, sensitive: bool) -> Self {
        self.sensitivity = Some(FieldSensitivity::from_sensitive(sensitive));
        self
    }
}

impl<'a, I> FieldEncoder for ValueRefsEncoder<'a, I>
where
    I: IntoIterator<Item = FieldValueRef<'a>>,
{
    fn encode<O>(self, output: &mut O) -> Result<(), InsertError>
    where
        O: FieldEncodeOutput,
    {
        for value in self.values {
            match self.sensitivity {
                Some(sensitivity) => value.with_sensitivity(sensitivity).encode(output)?,
                None => value.encode(output)?,
            }
        }
        Ok(())
    }
}

impl FieldEncoder for FieldValue {
    fn encode<O>(self, output: &mut O) -> Result<(), InsertError>
    where
        O: FieldEncodeOutput,
    {
        output.push_value(self)
    }
}

impl FieldEncoder for EncodedValues {
    fn encode<O>(self, output: &mut O) -> Result<(), InsertError>
    where
        O: FieldEncodeOutput,
    {
        for value in self {
            output.push_value(value)?;
        }
        Ok(())
    }
}

/// Encodes one unsigned integer as a decimal field value.
///
/// The [`FieldEncodeOutput`] example encodes an integer and inspects its
/// decimal bytes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct U64Encoder(u64);

impl U64Encoder {
    /// Creates an unsigned decimal encoder.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
}

impl FieldEncoder for U64Encoder {
    fn encode<O>(self, output: &mut O) -> Result<(), InsertError>
    where
        O: FieldEncodeOutput,
    {
        output.push_u64(self.0)
    }
}
