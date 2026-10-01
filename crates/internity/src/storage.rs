// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared CSR string storage helpers — the crate's single home for unchecked
//! UTF-8 reconstruction.
//!
//! Both interners ([`LocalLexicon`](crate::LocalLexicon) and each
//! [`ThreadedLexicon`](crate::ThreadedLexicon) shard) and their frozen
//! readers store strings the same way: every interned string's bytes are appended
//! to one contiguous `bytes` buffer, and a `u32` `offsets` table records the
//! boundaries CSR-style — `offsets[i]` is the start and `offsets[i + 1]` the end of
//! the `i`-th string, with a leading `0` sentinel so it holds `len() + 1` entries.
//!
//! Because every byte range in `offsets` was produced by appending a `&str`, the
//! bytes it spans are always valid UTF-8. These helpers exploit that to skip
//! re-validation (and, unlike slicing a `&str`, the UTF-8 char-boundary checks that
//! `str` range-indexing performs), which is the crate's fastest resolve path.
//!
//! This is the *only* module that uses `unsafe`. Given valid interner-produced
//! storage, [`resolve`] is memory-safe for any `index` and returns `None` when it
//! is out of range. The hot-path [`str_at`] helper additionally requires an
//! in-range index from a same-storage dedup handle or from a bounded iterator
//! position in `0..offsets.len() - 1`. Callers must uphold that offsets are
//! monotonic, remain within `bytes`, and delimit valid UTF-8 — guaranteed by
//! construction because ranges are recorded only when appending a `&str`.

/// Computes the end offset before appending bytes to CSR storage.
pub(crate) fn checked_end(current: usize, appended: usize) -> Option<u32> {
    current.checked_add(appended).and_then(|end| u32::try_from(end).ok())
}

/// Reconstructs the string at an **in-range** dense/local `index`
/// (`bytes[offsets[index]..offsets[index + 1]]`).
///
/// `index` must come from this storage's dedup table, or be a bounded iterator
/// position in `0..offsets.len() - 1`.
#[inline]
pub(crate) fn str_at<'a>(offsets: &[u32], bytes: &'a [u8], index: usize) -> &'a str {
    // SAFETY: a same-storage dedup handle or a bounded iterator position
    // establishes index < offsets.len() - 1.
    let start = unsafe { *offsets.get_unchecked(index) as usize };
    // SAFETY: that bound also establishes index + 1 < offsets.len().
    let end = unsafe { *offsets.get_unchecked(index + 1) as usize };
    // SAFETY: offsets are monotonic and end at `bytes.len()`.
    let stored = unsafe { bytes.get_unchecked(start..end) };
    // SAFETY: each stored range was appended from a `&str`.
    unsafe { core::str::from_utf8_unchecked(stored) }
}

/// Resolves a possibly-out-of-range dense/local `index`, returning `None` if it
/// does not name a stored string.
///
/// Used on the public `resolve` / `try_resolve` paths, which must tolerate foreign
/// or crafted handles.
#[inline]
pub(crate) fn resolve<'a>(offsets: &[u32], bytes: &'a [u8], index: usize) -> Option<&'a str> {
    let end_index = index.checked_add(1)?;
    let end = *offsets.get(end_index)? as usize;
    // SAFETY: finding `index + 1` above proves that `index` exists.
    let start = unsafe { *offsets.get_unchecked(index) as usize };
    // SAFETY: offsets are monotonic and end at `bytes.len()`.
    let stored = unsafe { bytes.get_unchecked(start..end) };
    // SAFETY: each stored range was appended from a `&str`.
    Some(unsafe { core::str::from_utf8_unchecked(stored) })
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    #[test]
    fn checked_end_rejects_overflow_and_truncation() {
        assert_eq!(super::checked_end(u32::MAX as usize - 1, 1), Some(u32::MAX));
        assert_eq!(super::checked_end(u32::MAX as usize - 1, 2), None);
        assert_eq!(super::checked_end(usize::MAX, 1), None);
    }

    #[test]
    fn resolve_rejects_an_overflowing_index() {
        assert_eq!(super::resolve(&[0], b"", usize::MAX), None);
    }
}
