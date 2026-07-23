// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use core::fmt::{self, Display, Write as _};

use super::{Error, ErrorKind, QueryLimits};

macro_rules! unsigned_pair_methods {
    ($($method:ident: $ty:ty),+ $(,)?) => {
        $(
            #[doc(hidden)]
            pub fn $method(
                &mut self,
                parameter: &'static str,
                encoded_parameter: &'static str,
                value: $ty,
            ) -> Result<(), Error> {
                self.start_pair(parameter, encoded_parameter)?;
                let mut buffer = itoa::Buffer::new();
                self.write_raw(Some(parameter), buffer.format(value))
            }
        )+
    };
}

macro_rules! signed_pair_methods {
    ($($method:ident: $ty:ty),+ $(,)?) => {
        $(
            #[doc(hidden)]
            pub fn $method(
                &mut self,
                parameter: &'static str,
                encoded_parameter: &'static str,
                value: $ty,
            ) -> Result<(), Error> {
                self.start_pair(parameter, encoded_parameter)?;
                let mut buffer = itoa::Buffer::new();
                self.write_raw(Some(parameter), buffer.format(value))
            }
        )+
    };
}

/// Streaming `application/x-www-form-urlencoded` encoder.
#[doc(hidden)]
#[derive(Debug)]
pub struct Encoder<'a, W> {
    output: &'a mut W,
    limits: QueryLimits,
    written: usize,
    first: bool,
}

impl<'a, W: fmt::Write> Encoder<'a, W> {
    unsigned_pair_methods!(
        pair_u8: u8,
        pair_u16: u16,
        pair_u32: u32,
        pair_u64: u64,
        pair_u128: u128,
    );
    signed_pair_methods!(
        pair_i8: i8,
        pair_i16: i16,
        pair_i32: i32,
        pair_i64: i64,
        pair_i128: i128,
    );

    #[doc(hidden)]
    pub fn pair_usize(&mut self, parameter: &'static str, encoded_parameter: &'static str, value: usize) -> Result<(), Error> {
        self.start_pair(parameter, encoded_parameter)?;
        let mut buffer = itoa::Buffer::new();
        self.write_raw(Some(parameter), buffer.format(value))
    }

    #[doc(hidden)]
    pub fn pair_isize(&mut self, parameter: &'static str, encoded_parameter: &'static str, value: isize) -> Result<(), Error> {
        self.start_pair(parameter, encoded_parameter)?;
        let mut buffer = itoa::Buffer::new();
        self.write_raw(Some(parameter), buffer.format(value))
    }

    /// Creates an encoder writing to `output`.
    #[must_use]
    pub fn new(output: &'a mut W, limits: QueryLimits) -> Self {
        Self {
            output,
            limits,
            written: 0,
            first: true,
        }
    }

    /// Writes one string-valued pair.
    ///
    /// # Errors
    ///
    /// Returns an error when the length limit or destination writer rejects
    /// output.
    pub fn pair_str(&mut self, parameter: &'static str, encoded_parameter: &'static str, value: &str) -> Result<(), Error> {
        self.start_pair(parameter, encoded_parameter)?;
        self.write_encoded(parameter, value)
    }

    /// Writes one displayable pair without allocating a temporary string.
    ///
    /// # Errors
    ///
    /// Returns an error when formatting, the length limit, or the destination
    /// writer rejects output.
    pub fn pair_display(&mut self, parameter: &'static str, encoded_parameter: &'static str, value: &impl Display) -> Result<(), Error> {
        self.start_pair(parameter, encoded_parameter)?;
        let mut adapter = EncodingWriter {
            encoder: self,
            parameter,
            error: None,
        };
        if write!(&mut adapter, "{value}").is_err() {
            return Err(adapter
                .error
                .unwrap_or_else(|| Error::production(Some(parameter), ErrorKind::Format)));
        }
        Ok(())
    }

    #[doc(hidden)]
    pub fn pair_bool(&mut self, parameter: &'static str, encoded_parameter: &'static str, value: bool) -> Result<(), Error> {
        self.start_pair(parameter, encoded_parameter)?;
        self.write_raw(Some(parameter), if value { "true" } else { "false" })
    }

    fn start_pair(&mut self, parameter: &'static str, encoded_parameter: &'static str) -> Result<(), Error> {
        if !self.first {
            self.write_raw(Some(parameter), "&")?;
        }
        self.first = false;
        self.write_raw(Some(parameter), encoded_parameter)?;
        self.write_raw(Some(parameter), "=")
    }

    fn write_encoded(&mut self, parameter: &'static str, value: &str) -> Result<(), Error> {
        let bytes = value.as_bytes();
        let mut run_start = 0;
        for (index, &byte) in bytes.iter().enumerate() {
            if is_safe(byte) {
                continue;
            }
            if run_start < index {
                self.write_raw(Some(parameter), &value[run_start..index])?;
            }
            if byte == b' ' {
                self.write_raw(Some(parameter), "+")?;
            } else {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                let escaped = [b'%', HEX[(byte >> 4) as usize], HEX[(byte & 0x0F) as usize]];
                let escaped = core::str::from_utf8(&escaped).expect("percent escape is ASCII");
                self.write_raw(Some(parameter), escaped)?;
            }
            run_start = index + 1;
        }
        if run_start < value.len() {
            self.write_raw(Some(parameter), &value[run_start..])?;
        }
        Ok(())
    }

    fn write_raw(&mut self, parameter: Option<&'static str>, value: &str) -> Result<(), Error> {
        let Some(new_length) = self.written.checked_add(value.len()) else {
            return Err(Error::production(parameter, ErrorKind::TooLong));
        };
        if new_length > self.limits.max_encoded_length {
            return Err(Error::production(parameter, ErrorKind::TooLong));
        }
        self.output
            .write_str(value)
            .map_err(|_error| Error::production(parameter, ErrorKind::Output))?;
        self.written = new_length;
        Ok(())
    }
}

struct EncodingWriter<'a, 'b, W> {
    encoder: &'a mut Encoder<'b, W>,
    parameter: &'static str,
    error: Option<Error>,
}

impl<W: fmt::Write> fmt::Write for EncodingWriter<'_, '_, W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.encoder.write_encoded(self.parameter, s).map_err(|error| {
            self.error = Some(error);
            fmt::Error
        })
    }
}

const fn is_safe(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'*' | b'-' | b'.' | b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RejectEmptyWrites(alloc::string::String);

    impl fmt::Write for RejectEmptyWrites {
        #[cfg_attr(coverage_nightly, coverage(off))]
        fn write_str(&mut self, s: &str) -> fmt::Result {
            if s.is_empty() {
                return Err(fmt::Error);
            }
            self.0.push_str(s);
            Ok(())
        }
    }

    #[test]
    fn written_length_overflow_is_reported() {
        let mut output = alloc::string::String::new();
        let mut encoder = Encoder::new(&mut output, QueryLimits::UNLIMITED);
        encoder.written = usize::MAX;
        let error = encoder.write_raw(Some("value"), "x").expect_err("length overflow");
        assert_eq!(error.parameter(), Some("value"));
        assert_eq!(error.kind(), ErrorKind::TooLong);
    }

    #[test]
    fn exact_output_limit_is_accepted_without_empty_writes() {
        let mut output = RejectEmptyWrites(alloc::string::String::new());
        let limits = QueryLimits {
            max_encoded_length: 9,
            ..QueryLimits::UNLIMITED
        };
        let mut encoder = Encoder::new(&mut output, limits);
        encoder.write_encoded("value", "/abc/").expect("exact-length output writes");
        assert_eq!(output.0, "%2Fabc%2F");
    }
}
