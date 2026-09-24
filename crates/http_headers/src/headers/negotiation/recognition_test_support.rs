// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use super::shared::try_plain_items;
use crate::DecodeError;

/// Builds every string up to `length` bytes over `alphabet`.
/// Miri keeps every one/two-byte case and samples longer token/range spellings.
pub(super) fn exhaustive(alphabet: &[u8], length: usize) -> Vec<Vec<u8>> {
    const MIRI_CONTINUATIONS: &[u8] = b"a*qQ01/-";

    let mut all = vec![Vec::new()];
    let mut frontier = vec![Vec::new()];
    for depth in 0..length {
        let mut next = Vec::new();
        for prefix in &frontier {
            if cfg!(miri)
                && depth >= 2
                && (!matches!(prefix.first(), Some(b'a' | b'q' | b'Q' | b'*'))
                    || !prefix.iter().skip(1).all(|byte| MIRI_CONTINUATIONS.contains(byte)))
            {
                continue;
            }
            for byte in alphabet {
                if cfg!(miri) && depth >= 2 && !MIRI_CONTINUATIONS.contains(byte) {
                    continue;
                }
                let mut candidate = prefix.clone();
                candidate.push(*byte);
                next.push(candidate);
            }
        }
        all.extend_from_slice(&next);
        frontier = next;
    }
    all
}

/// Builds every concatenation of up to `count` of the given fragments.
/// Miri keeps all pairs, then samples quality, parameter, separator and quote continuations.
pub(super) fn fragment_lines(fragments: &[&str], count: usize) -> Vec<Vec<u8>> {
    const MIRI_CONTINUATIONS: &[&str] = &[";q=0.9", ";q=1.001", ";q=0.1234", ";x", ";x=y", ",", " ", "\t", "\"", "\\"];

    let mut lines = vec![Vec::new()];
    for depth in 0..count {
        let mut next = Vec::new();
        for prefix in &lines {
            if cfg!(miri) && depth >= 2 && !fragments.iter().take(3).any(|fragment| prefix.starts_with(fragment.as_bytes())) {
                continue;
            }
            for fragment in fragments {
                if cfg!(miri) && depth >= 2 && !MIRI_CONTINUATIONS.contains(fragment) {
                    continue;
                }
                let mut candidate = prefix.clone();
                candidate.extend_from_slice(fragment.as_bytes());
                next.push(candidate);
            }
        }
        lines.extend_from_slice(&next);
    }
    lines
}

pub(super) fn assert_recognition_is_sound(
    lines: &[Vec<u8>],
    recognize: fn(&[u8]) -> bool,
    strict: fn(&[u8]) -> Result<(), DecodeError>,
    relaxed: fn(&[u8]) -> Result<(), DecodeError>,
) -> usize {
    let mut recognized = 0;
    for line in lines {
        if !recognize(line) {
            continue;
        }
        recognized += 1;
        let shown = String::from_utf8_lossy(line);
        assert!(
            matches!(try_plain_items(line, b',', true, strict), Ok(true)),
            "recognized line {shown:?} must satisfy the strict member grammar"
        );
        assert!(
            matches!(try_plain_items(line, b',', true, relaxed), Ok(true)),
            "recognized line {shown:?} must satisfy the relaxed member grammar"
        );
    }
    recognized
}
