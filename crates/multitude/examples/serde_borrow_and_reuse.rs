// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Borrow strings directly from input when possible and reuse growable arena
//! buffers across deserialization operations.

#![allow(clippy::missing_panics_doc, reason = "example code")]

use multitude::{Arena, Box, Cow};

fn main() -> serde_json::Result<()> {
    let arena = Arena::new();

    let borrowed: Box<Cow<'_, str>> = arena.deserialize_json(br#""already decoded""#)?;
    assert!(borrowed.is_borrowed());

    // JSON escapes require decoding, so Cow copies the final string into
    // arena storage instead of borrowing a temporary parser buffer.
    let owned: Box<Cow<'_, str>> = arena.deserialize_json(br#""decoded\u0020text""#)?;
    assert!(!owned.is_borrowed());
    assert_eq!(&**owned, "decoded text");

    let mut text = arena.alloc_string_with_capacity(64);
    text.push_str("old value");
    let text_allocation = text.as_ptr();
    let mut text_input = serde_json::Deserializer::from_str(r#""replacement""#);
    text.deserialize_reusing(&mut text_input)?;
    assert_eq!(text.as_str(), "replacement");
    assert_eq!(text.as_ptr(), text_allocation);

    let mut numbers = arena.alloc_vec_with_capacity(8);
    numbers.extend([10_u64, 20, 30]);
    let numbers_allocation = numbers.as_ptr();
    let mut number_input = serde_json::Deserializer::from_str("[1,2,3,4]");
    numbers.deserialize_reusing(&mut number_input)?;
    assert_eq!(numbers.as_slice(), &[1, 2, 3, 4]);
    assert_eq!(numbers.as_ptr(), numbers_allocation);

    Ok(())
}
