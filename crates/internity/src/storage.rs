// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared CSR string storage helpers — the crate's single home for unchecked
//! UTF-8 reconstruction.
//!
//! Both interners ([`Lexicon`](crate::Lexicon), and each
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
//! in-range index produced by the storage's own dedup table. Callers must uphold
//! that offsets are monotonic, remain within `bytes`, and delimit valid UTF-8 —
//! guaranteed by construction because ranges are recorded only when appending a
//! `&str`.

/// Reconstructs the string at an **in-range** dense/local `index`
/// (`bytes[offsets[index]..offsets[index + 1]]`).
///
/// Used on the hot interning-compare path, where `index` comes from a handle the
/// table just produced and is therefore always valid.
#[inline]
pub(crate) fn str_at<'a>(offsets: &[u32], bytes: &'a [u8], index: usize) -> &'a str {
    // SAFETY: callers only pass an index obtained from this storage's dedup table.
    let start = unsafe { *offsets.get_unchecked(index) as usize };
    // SAFETY: the same table-produced index guarantees the adjacent end exists.
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

#[cfg(test)]
mod tests {
    #[test]
    fn resolve_rejects_an_overflowing_index() {
        assert_eq!(super::resolve(&[0], b"", usize::MAX), None);
    }
}
