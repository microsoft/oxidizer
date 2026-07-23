// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use alloc::vec::Vec;

use bytes::Bytes;

use super::Body;

mod sealed {
    pub trait Integer {}
    pub trait Slot {}
}

/// An integer accepted by a generated JSON number slot.
#[doc(hidden)]
pub trait Integer: sealed::Integer + Copy {
    fn parts(self) -> (bool, u128);
}

macro_rules! unsigned_integers {
    ($($integer:ty),+ $(,)?) => {
        $(
            impl sealed::Integer for $integer {}

            impl Integer for $integer {
                fn parts(self) -> (bool, u128) {
                    (false, u128::from(self))
                }
            }
        )+
    };
}

macro_rules! signed_integers {
    ($($integer:ty),+ $(,)?) => {
        $(
            impl sealed::Integer for $integer {}

            impl Integer for $integer {
                fn parts(self) -> (bool, u128) {
                    let value = i128::from(self);
                    (value.is_negative(), value.unsigned_abs())
                }
            }
        )+
    };
}

unsigned_integers!(u8, u16, u32, u64);
signed_integers!(i8, i16, i32, i64);

impl sealed::Integer for usize {}

impl Integer for usize {
    fn parts(self) -> (bool, u128) {
        (false, self as u128)
    }
}

impl sealed::Integer for isize {}

impl Integer for isize {
    fn parts(self) -> (bool, u128) {
        let value = self as i128;
        (value.is_negative(), value.unsigned_abs())
    }
}

impl sealed::Integer for u128 {}

impl Integer for u128 {
    fn parts(self) -> (bool, u128) {
        (false, self)
    }
}

impl sealed::Integer for i128 {}

impl Integer for i128 {
    fn parts(self) -> (bool, u128) {
        (self.is_negative(), self.unsigned_abs())
    }
}

/// A domain-encoded response-template slot.
#[doc(hidden)]
pub trait Slot: sealed::Slot {
    fn encoded_len(&self) -> usize;
    fn write_to(&self, output: &mut impl TemplateOutput);
}

/// A byte destination used by typed response-template slots.
#[doc(hidden)]
pub trait TemplateOutput {
    fn push_byte(&mut self, byte: u8);
    fn write_bytes(&mut self, bytes: &[u8]);
}

impl TemplateOutput for Vec<u8> {
    fn push_byte(&mut self, byte: u8) {
        self.push(byte);
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        self.extend_from_slice(bytes);
    }
}

#[cfg(feature = "bytesbuf")]
impl TemplateOutput for bytesbuf::BytesBuf {
    fn push_byte(&mut self, byte: u8) {
        self.put_byte(byte);
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        self.put_slice(bytes);
    }
}

/// A JSON integer slot.
#[doc(hidden)]
#[derive(Debug)]
pub struct JsonNumber<T>(T);

/// A JSON string slot.
#[doc(hidden)]
#[derive(Debug)]
pub struct JsonString<T>(T);

/// An HTML text-content slot.
#[doc(hidden)]
#[derive(Debug)]
pub struct HtmlText<T>(T);

/// An unescaped plain-text slot.
#[doc(hidden)]
#[derive(Debug)]
pub struct PlainText<T>(T);

/// Wraps an integer as one complete JSON number value.
#[doc(hidden)]
#[must_use]
pub fn json_number<T: Integer>(value: T) -> JsonNumber<T> {
    JsonNumber(value)
}

/// Wraps a string-like value as one complete escaped JSON string value.
#[doc(hidden)]
#[must_use]
pub fn json_string<T: AsRef<str>>(value: T) -> JsonString<T> {
    JsonString(value)
}

/// Wraps a string-like value as escaped HTML text content.
#[doc(hidden)]
#[must_use]
pub fn html_text<T: AsRef<str>>(value: T) -> HtmlText<T> {
    HtmlText(value)
}

/// Wraps a string-like value as unescaped plain text.
#[doc(hidden)]
#[must_use]
pub fn plain_text<T: AsRef<str>>(value: T) -> PlainText<T> {
    PlainText(value)
}

impl<T: Integer> sealed::Slot for JsonNumber<T> {}

impl<T: Integer> Slot for JsonNumber<T> {
    fn encoded_len(&self) -> usize {
        let (negative, magnitude) = self.0.parts();
        usize::from(negative) + decimal_length(magnitude)
    }

    fn write_to(&self, output: &mut impl TemplateOutput) {
        let (negative, magnitude) = self.0.parts();
        write_decimal(output, negative, magnitude);
    }
}

impl<T: AsRef<str>> sealed::Slot for JsonString<T> {}

impl<T: AsRef<str>> Slot for JsonString<T> {
    fn encoded_len(&self) -> usize {
        escaped_json_string_length(self.0.as_ref())
    }

    fn write_to(&self, output: &mut impl TemplateOutput) {
        write_json_string(output, self.0.as_ref());
    }
}

impl<T: AsRef<str>> sealed::Slot for HtmlText<T> {}

impl<T: AsRef<str>> Slot for HtmlText<T> {
    fn encoded_len(&self) -> usize {
        escaped_html_text_length(self.0.as_ref())
    }

    fn write_to(&self, output: &mut impl TemplateOutput) {
        write_html_text(output, self.0.as_ref());
    }
}

impl<T: AsRef<str>> sealed::Slot for PlainText<T> {}

impl<T: AsRef<str>> Slot for PlainText<T> {
    fn encoded_len(&self) -> usize {
        self.0.as_ref().len()
    }

    fn write_to(&self, output: &mut impl TemplateOutput) {
        output.write_bytes(self.0.as_ref().as_bytes());
    }
}

/// Returns one slot's exact encoded length.
#[doc(hidden)]
#[must_use]
pub fn slot_len(slot: &impl Slot) -> usize {
    slot.encoded_len()
}

/// Writes one already typed slot to the exactly sized output.
#[doc(hidden)]
pub fn write_slot(output: &mut impl TemplateOutput, slot: &impl Slot) {
    slot.write_to(output);
}

/// Adds fixed and dynamic encoded lengths with overflow checking.
#[doc(hidden)]
#[must_use]
pub fn total_length<const N: usize>(lengths: [usize; N]) -> usize {
    lengths.into_iter().fold(0, |total, length| {
        total
            .checked_add(length)
            .expect("static fragments plus encoded slots must fit in usize to allocate the response body")
    })
}

/// Creates the one exactly sized dynamic response allocation.
#[doc(hidden)]
#[inline]
#[must_use]
pub fn output(length: usize) -> Vec<u8> {
    Vec::with_capacity(length)
}

/// Finishes a dynamic response template after its single write pass.
#[doc(hidden)]
#[inline]
#[must_use]
pub fn finish(output: Vec<u8>, expected_length: usize) -> Body {
    debug_assert_eq!(output.len(), expected_length);
    Body::from(output)
}

/// Creates an allocation-free body from compile-time-only template text.
#[doc(hidden)]
#[inline]
#[must_use]
pub fn static_body(text: &'static str) -> Body {
    Body::from(Bytes::from_static(text.as_bytes()))
}

const fn decimal_length(mut value: u128) -> usize {
    let mut length = 1;
    while value >= 10 {
        value /= 10;
        length += 1;
    }
    length
}

// `itoa::Buffer` is the usual integer formatter, but enabling `itoa` for the
// otherwise minimal `response` feature solely for this sealed slot would widen
// that feature's dependency set. This fixed u128 buffer covers every integer
// primitive without allocation; boundary fixtures pin i128::MIN and u128::MAX.
fn write_decimal(output: &mut impl TemplateOutput, negative: bool, mut magnitude: u128) {
    let mut buffer = [0_u8; 39];
    let mut index = buffer.len();
    loop {
        index -= 1;
        buffer[index] = b'0' + (magnitude % 10) as u8;
        magnitude /= 10;
        if magnitude == 0 {
            break;
        }
    }
    if negative {
        output.push_byte(b'-');
    }
    output.write_bytes(&buffer[index..]);
}

// `serde_json::to_writer` is the usual JSON encoder. Using it here would either
// allocate an intermediate string or run the general serializer after a
// separate length pass, and would make the standalone `response` feature
// depend on Serde. This sealed string-value slot implements exactly
// serde_json's compact string escapes; differential fixtures cover every ASCII
// control and UTF-8.
fn escaped_json_string_length(value: &str) -> usize {
    value.bytes().fold(2, |length, byte| {
        let encoded = match byte {
            b'"' | b'\\' | b'\x08' | b'\t' | b'\n' | b'\x0c' | b'\r' => 2,
            b'\x00'..=b'\x1f' => 6,
            _ => 1,
        };

        length
            .checked_add(encoded)
            .expect("an escaped JSON string must fit in usize to allocate the response body")
    })
}

fn write_json_string(output: &mut impl TemplateOutput, value: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    output.push_byte(b'"');
    for byte in value.bytes() {
        match byte {
            b'"' => output.write_bytes(br#"\""#),
            b'\\' => output.write_bytes(br"\\"),
            b'\x08' => output.write_bytes(br"\b"),
            b'\t' => output.write_bytes(br"\t"),
            b'\n' => output.write_bytes(br"\n"),
            b'\x0c' => output.write_bytes(br"\f"),
            b'\r' => output.write_bytes(br"\r"),
            b'\x00'..=b'\x1f' => {
                output.write_bytes(br"\u00");
                output.push_byte(HEX[(byte >> 4) as usize]);
                output.push_byte(HEX[(byte & 0x0f) as usize]);
            }
            _ => output.push_byte(byte),
        }
    }
    output.push_byte(b'"');
}

// A general HTML serializer is unnecessary for one text-content context and
// would add a dependency or an intermediate allocation. This sealed slot
// escapes the five HTML-sensitive characters, even though quotes are
// conservative in text content; callers cannot select a raw dynamic HTML slot.
fn escaped_html_text_length(value: &str) -> usize {
    value.bytes().fold(0, |length, byte| {
        let encoded = match byte {
            b'&' => b"&amp;".len(),
            b'<' => b"&lt;".len(),
            b'>' => b"&gt;".len(),
            b'"' => b"&quot;".len(),
            b'\'' => b"&#39;".len(),
            _ => 1,
        };

        length
            .checked_add(encoded)
            .expect("escaped HTML text must fit in usize to allocate the response body")
    })
}

fn write_html_text(output: &mut impl TemplateOutput, value: &str) {
    for byte in value.bytes() {
        match byte {
            b'&' => output.write_bytes(b"&amp;"),
            b'<' => output.write_bytes(b"&lt;"),
            b'>' => output.write_bytes(b"&gt;"),
            b'"' => output.write_bytes(b"&quot;"),
            b'\'' => output.write_bytes(b"&#39;"),
            _ => output.push_byte(byte),
        }
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! __routerama_json_template_slot {
    (number, $value:expr) => {
        $crate::response::__template::json_number($value)
    };
    (string, $value:expr) => {
        $crate::response::__template::json_string($value)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __routerama_html_template_slot {
    (text, $value:expr) => {
        $crate::response::__template::html_text($value)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __routerama_template_part_len {
    ($literal:literal) => {
        $literal.len()
    };
    ($slot:ident) => {
        $crate::response::__template::slot_len(&$slot)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __routerama_template_part_write {
    ($output:ident, $literal:literal) => {
        $output.extend_from_slice($literal.as_bytes())
    };
    ($output:ident, $slot:ident) => {
        $crate::response::__template::write_slot(&mut $output, &$slot)
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __routerama_json_body_template {
    ($($literal:literal),+ $(,)?) => {
        $crate::response::__template::static_body(concat!($($literal),+))
    };
    (
        $($name:ident = $kind:ident($value:expr)),+ $(,)?;
        $($part:tt),+ $(,)?
    ) => {{
        $(let $name = $crate::__routerama_json_template_slot!($kind, $value);)+
        let length = $crate::response::__template::total_length([
            $($crate::__routerama_template_part_len!($part)),+
        ]);
        let mut output = $crate::response::__template::output(length);
        $($crate::__routerama_template_part_write!(output, $part);)+
        $crate::response::__template::finish(output, length)
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __routerama_html_body_template {
    ($($literal:literal),+ $(,)?) => {
        $crate::response::__template::static_body(concat!($($literal),+))
    };
    (
        $($name:ident = text($value:expr)),+ $(,)?;
        $($part:tt),+ $(,)?
    ) => {{
        $(let $name = $crate::__routerama_html_template_slot!(text, $value);)+
        let length = $crate::response::__template::total_length([
            $($crate::__routerama_template_part_len!($part)),+
        ]);
        let mut output = $crate::response::__template::output(length);
        $($crate::__routerama_template_part_write!(output, $part);)+
        $crate::response::__template::finish(output, length)
    }};
}
