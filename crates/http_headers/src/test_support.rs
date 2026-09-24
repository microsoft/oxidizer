// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Native substitutions remain exhaustive; Miri samples grammar classes and ASCII case changes.

#[cfg(miri)]
const BYTE_CLASSES: &[u8] = b"\0\t\n\x0b\x0c\r\x1f !\"*+,-./0123456789:;=?@AZ[\\]_`az{\x7f\x80\xff";

#[cfg(miri)]
pub(crate) fn is_byte_case(byte: u8, original: u8) -> bool {
    BYTE_CLASSES.contains(&byte) || byte.eq_ignore_ascii_case(&original)
}

#[cfg(miri)]
pub(crate) fn byte_cases(original: u8) -> impl Iterator<Item = u8> {
    let lowercase = original.to_ascii_lowercase();
    let uppercase = original.to_ascii_uppercase();
    BYTE_CLASSES
        .iter()
        .copied()
        .chain((!BYTE_CLASSES.contains(&original)).then_some(original))
        .chain((lowercase != original && !BYTE_CLASSES.contains(&lowercase)).then_some(lowercase))
        .chain((uppercase != original && !BYTE_CLASSES.contains(&uppercase)).then_some(uppercase))
}

#[cfg(not(miri))]
pub(crate) fn byte_cases(_original: u8) -> impl Iterator<Item = u8> {
    u8::MIN..=u8::MAX
}

#[cfg(miri)]
pub(crate) fn substitution_bytes(original: u8, position: usize, length: usize) -> impl Iterator<Item = u8> {
    assert!(position < length, "the substitution position must be inside the input");
    byte_cases(original).filter(move |&byte| {
        if byte == original {
            return position == 0;
        }
        // Keep structural mutations at every position, and distribute the remaining classes across the literal.
        byte.eq_ignore_ascii_case(&original)
            || matches!(byte, 0 | b' ' | b'"' | b'\\' | b',' | b';' | 0x7f | 0x80)
            || original.is_ascii_digit() && byte.is_ascii_digit()
            || usize::from(byte) % length == position
    })
}

#[cfg(not(miri))]
pub(crate) fn substitution_bytes(original: u8, _position: usize, _length: usize) -> impl Iterator<Item = u8> {
    byte_cases(original)
}
