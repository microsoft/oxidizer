// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Growable container builders on [`Arena`]: [`String`] and [`Vec`].
//!
//! All public methods are documented on [`Arena`] itself; this file
//! groups the family together to keep the central `mod.rs` smaller.

use allocator_api2::alloc::Allocator;

use super::Arena;
use crate::AllocError;
use crate::arena::ExpectAlloc;
use crate::strings::{FromUtf16Error, String};
use crate::vec::Vec;

impl<A: Allocator + Clone> Arena<A> {
    /// Create a new, empty growable [`String`](crate::strings::String) backed by this
    /// arena. No allocation is performed until the first push.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut s = arena.alloc_string();
    /// s.push_str("hello");
    /// assert_eq!(&*s, "hello");
    /// ```
    #[must_use]
    #[inline]
    pub const fn alloc_string(&self) -> String<'_, A> {
        String::new_in(self)
    }

    /// Create a new growable arena-backed [`String`](crate::strings::String) with capacity.
    ///
    /// At least `cap` bytes are pre-allocated.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails. Use
    /// [`Self::try_alloc_string_with_capacity`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut s = arena.alloc_string_with_capacity(64);
    /// s.push_str("preallocated");
    /// assert!(s.capacity() >= 64);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_string_with_capacity(&self, cap: usize) -> String<'_, A> {
        String::with_capacity_in(cap, self)
    }

    /// Fallible variant of [`Self::alloc_string_with_capacity`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_string_with_capacity(16) else {
    ///     panic!("allocation failed");
    /// };
    /// assert!(value.capacity() >= 16);
    /// ```
    #[inline]
    pub fn try_alloc_string_with_capacity(&self, cap: usize) -> Result<String<'_, A>, AllocError> {
        String::try_with_capacity_in(cap, self)
    }

    /// Validate `bytes` as UTF-8 and copy them into an arena [`String`].
    ///
    /// This is the arena-bound analog of
    /// [`std::string::String::from_utf8`] (taking a borrowed slice rather
    /// than an owning `Vec<u8>`).
    ///
    /// # Errors
    ///
    /// Returns a [`Utf8Error`](core::str::Utf8Error) if `bytes` is not valid
    /// UTF-8.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails. Allocation failure is reported
    /// via a panic, not the returned `Result`.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.alloc_string_from_utf8(b"hello") else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, "hello");
    /// ```
    pub fn alloc_string_from_utf8(&self, bytes: impl AsRef<[u8]>) -> Result<String<'_, A>, core::str::Utf8Error> {
        let bytes = bytes.as_ref();
        Ok(String::from_str_in(core::str::from_utf8(bytes)?, self))
    }

    /// Copy `bytes` into an arena [`String`], replacing invalid UTF-8 with `U+FFFD`.
    ///
    /// This is the arena-bound analog of
    /// [`std::string::String::from_utf8_lossy`].
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_string_from_utf8_lossy(b"a\xFFb");
    /// assert_eq!(&*value, "a\u{FFFD}b");
    /// ```
    #[must_use]
    pub fn alloc_string_from_utf8_lossy(&self, bytes: impl AsRef<[u8]>) -> String<'_, A> {
        self.try_alloc_string_from_utf8_lossy(bytes).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_string_from_utf8_lossy`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_string_from_utf8_lossy(b"x\xFF") else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, "x\u{FFFD}");
    /// ```
    pub fn try_alloc_string_from_utf8_lossy(&self, bytes: impl AsRef<[u8]>) -> Result<String<'_, A>, AllocError> {
        let bytes = bytes.as_ref();
        // Decode directly into the arena string; no intermediate global
        // allocation (unlike `str::from_utf8_lossy`'s owned `Cow`).
        let mut out = self.try_alloc_string_with_capacity(bytes.len())?;
        for chunk in bytes.utf8_chunks() {
            out.try_push_str(chunk.valid())?;
            if !chunk.invalid().is_empty() {
                out.try_push(char::REPLACEMENT_CHARACTER)?;
            }
        }
        Ok(out)
    }

    /// Copy presumed UTF-8 `bytes` into an arena [`String`] without validation.
    ///
    /// This is the arena-bound analog of
    /// [`std::string::String::from_utf8_unchecked`].
    ///
    /// # Safety
    ///
    /// `bytes` must be valid UTF-8.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// // SAFETY: the byte string is valid UTF-8.
    /// let value = unsafe { arena.alloc_string_from_utf8_unchecked(b"valid") };
    /// assert_eq!(&*value, "valid");
    /// ```
    #[must_use]
    pub unsafe fn alloc_string_from_utf8_unchecked(&self, bytes: impl AsRef<[u8]>) -> String<'_, A> {
        // SAFETY: the caller guarantees `bytes` is valid UTF-8.
        unsafe { self.try_alloc_string_from_utf8_unchecked(bytes) }.expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_string_from_utf8_unchecked`].
    ///
    /// # Safety
    ///
    /// `bytes` must be valid UTF-8.
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// // SAFETY: the byte string is valid UTF-8.
    /// let Ok(value) = (unsafe { arena.try_alloc_string_from_utf8_unchecked(b"valid") }) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, "valid");
    /// ```
    pub unsafe fn try_alloc_string_from_utf8_unchecked(&self, bytes: impl AsRef<[u8]>) -> Result<String<'_, A>, AllocError> {
        let bytes = bytes.as_ref();
        // SAFETY: the caller guarantees `bytes` is valid UTF-8.
        String::try_from_str_in(unsafe { core::str::from_utf8_unchecked(bytes) }, self)
    }

    /// Decode native-endian UTF-16 `units` into a fresh arena [`String`].
    /// The arena-bound analog of [`std::string::String::from_utf16`].
    ///
    /// # Errors
    ///
    /// Returns a [`DecodeUtf16Error`](core::char::DecodeUtf16Error) on the
    /// first unpaired surrogate.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails. Allocation failure is reported
    /// via a panic, not the returned `Result`.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.alloc_string_from_utf16([0x0041, 0x03A9]) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, "AΩ");
    /// ```
    pub fn alloc_string_from_utf16(&self, units: impl AsRef<[u16]>) -> Result<String<'_, A>, core::char::DecodeUtf16Error> {
        let units = units.as_ref();
        let mut out = self.alloc_string_with_capacity(units.len());
        for unit in char::decode_utf16(units.iter().copied()) {
            out.push(unit?);
        }
        Ok(out)
    }

    /// Decode UTF-16 `units` into an arena [`String`], replacing unpaired surrogates.
    ///
    /// Unpaired surrogates become `U+FFFD`. This is the arena-bound analog
    /// of [`std::string::String::from_utf16_lossy`].
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_string_from_utf16_lossy([0x0041, 0xD800]);
    /// assert_eq!(&*value, "A\u{FFFD}");
    /// ```
    #[must_use]
    pub fn alloc_string_from_utf16_lossy(&self, units: impl AsRef<[u16]>) -> String<'_, A> {
        self.try_alloc_string_from_utf16_lossy(units).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_string_from_utf16_lossy`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_string_from_utf16_lossy([0x0042, 0xD800]) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, "B\u{FFFD}");
    /// ```
    pub fn try_alloc_string_from_utf16_lossy(&self, units: impl AsRef<[u16]>) -> Result<String<'_, A>, AllocError> {
        let units = units.as_ref();
        let mut out = self.try_alloc_string_with_capacity(units.len())?;
        for unit in char::decode_utf16(units.iter().copied()) {
            out.try_push(unit.unwrap_or(char::REPLACEMENT_CHARACTER))?;
        }
        Ok(out)
    }

    /// Decode little-endian UTF-16 `bytes` into a fresh arena [`String`]. The
    /// arena-bound analog of [`std::string::String::from_utf16le`].
    ///
    /// # Errors
    ///
    /// Returns a [`FromUtf16Error`](FromUtf16Error) if `bytes`
    /// has an odd length or contains an unpaired surrogate.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails. Allocation failure is reported
    /// via a panic, not the returned `Result`.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.alloc_string_from_utf16le([b'A', 0, b'B', 0]) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, "AB");
    /// ```
    pub fn alloc_string_from_utf16le(&self, bytes: impl AsRef<[u8]>) -> Result<String<'_, A>, FromUtf16Error> {
        self.alloc_string_from_utf16_bytes(bytes.as_ref(), false)
    }

    /// Decode big-endian UTF-16 `bytes` into a fresh arena [`String`]. The
    /// arena-bound analog of [`std::string::String::from_utf16be`].
    ///
    /// # Errors
    ///
    /// Returns a [`FromUtf16Error`](FromUtf16Error) if `bytes`
    /// has an odd length or contains an unpaired surrogate.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails. Allocation failure is reported
    /// via a panic, not the returned `Result`.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.alloc_string_from_utf16be([0, b'A', 0, b'B']) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, "AB");
    /// ```
    pub fn alloc_string_from_utf16be(&self, bytes: impl AsRef<[u8]>) -> Result<String<'_, A>, FromUtf16Error> {
        self.alloc_string_from_utf16_bytes(bytes.as_ref(), true)
    }

    /// Decode little-endian UTF-16 `bytes` into a fresh arena [`String`] (lossy).
    ///
    /// Odd trailing bytes and unpaired surrogates are replaced with `U+FFFD`. The
    /// arena-bound analog of [`std::string::String::from_utf16le_lossy`].
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_string_from_utf16le_lossy([b'A', 0, 0]);
    /// assert_eq!(&*value, "A\u{FFFD}");
    /// ```
    #[must_use]
    pub fn alloc_string_from_utf16le_lossy(&self, bytes: impl AsRef<[u8]>) -> String<'_, A> {
        self.alloc_string_from_utf16_bytes_lossy(bytes.as_ref(), false)
    }

    /// Fallible variant of [`Self::alloc_string_from_utf16le_lossy`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_string_from_utf16le_lossy([b'A', 0]) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, "A");
    /// ```
    pub fn try_alloc_string_from_utf16le_lossy(&self, bytes: impl AsRef<[u8]>) -> Result<String<'_, A>, AllocError> {
        self.try_alloc_string_from_utf16_bytes_lossy(bytes.as_ref(), false)
    }

    /// Decode big-endian UTF-16 `bytes` into a fresh arena [`String`] (lossy).
    ///
    /// Odd trailing bytes and unpaired surrogates are replaced with `U+FFFD`. The
    /// arena-bound analog of [`std::string::String::from_utf16be_lossy`].
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let value = arena.alloc_string_from_utf16be_lossy([0, b'A', 0]);
    /// assert_eq!(&*value, "A\u{FFFD}");
    /// ```
    #[must_use]
    pub fn alloc_string_from_utf16be_lossy(&self, bytes: impl AsRef<[u8]>) -> String<'_, A> {
        self.alloc_string_from_utf16_bytes_lossy(bytes.as_ref(), true)
    }

    /// Fallible variant of [`Self::alloc_string_from_utf16be_lossy`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_string_from_utf16be_lossy([0, b'B']) else {
    ///     panic!("allocation failed");
    /// };
    /// assert_eq!(&*value, "B");
    /// ```
    pub fn try_alloc_string_from_utf16be_lossy(&self, bytes: impl AsRef<[u8]>) -> Result<String<'_, A>, AllocError> {
        self.try_alloc_string_from_utf16_bytes_lossy(bytes.as_ref(), true)
    }

    /// Shared body for the byte-oriented UTF-16 constructors. `big_endian`
    /// selects the byte order used to assemble each `u16` code unit.
    #[expect(
        clippy::map_err_ignore,
        reason = "FromUtf16Error is intentionally opaque; the DecodeUtf16Error carries no extra recoverable detail"
    )]
    fn alloc_string_from_utf16_bytes(&self, bytes: &[u8], big_endian: bool) -> Result<String<'_, A>, FromUtf16Error> {
        if !bytes.len().is_multiple_of(2) {
            return Err(FromUtf16Error::new());
        }
        let mut out = self.alloc_string_with_capacity(bytes.len() / 2);
        let units = bytes.chunks_exact(2).map(|pair| {
            let raw = [pair[0], pair[1]];
            if big_endian {
                u16::from_be_bytes(raw)
            } else {
                u16::from_le_bytes(raw)
            }
        });
        for unit in char::decode_utf16(units) {
            out.push(unit.map_err(|_| FromUtf16Error::new())?);
        }
        Ok(out)
    }

    /// Shared body for the lossy byte-oriented UTF-16 constructors.
    fn alloc_string_from_utf16_bytes_lossy(&self, bytes: &[u8], big_endian: bool) -> String<'_, A> {
        self.try_alloc_string_from_utf16_bytes_lossy(bytes, big_endian).expect_alloc()
    }

    /// Fallible variant of [`Self::alloc_string_from_utf16_bytes_lossy`].
    fn try_alloc_string_from_utf16_bytes_lossy(&self, bytes: &[u8], big_endian: bool) -> Result<String<'_, A>, AllocError> {
        let mut out = self.try_alloc_string_with_capacity(bytes.len() / 2 + 1)?;
        let units = bytes.chunks_exact(2).map(|pair| {
            let raw = [pair[0], pair[1]];
            if big_endian {
                u16::from_be_bytes(raw)
            } else {
                u16::from_le_bytes(raw)
            }
        });
        for unit in char::decode_utf16(units) {
            out.try_push(unit.unwrap_or(char::REPLACEMENT_CHARACTER))?;
        }
        if !bytes.len().is_multiple_of(2) {
            out.try_push(char::REPLACEMENT_CHARACTER)?;
        }
        Ok(out)
    }

    /// Create a new, empty growable [`Vec`](crate::vec::Vec) backed by this arena.
    /// No allocation is performed until the first push.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut v = arena.alloc_vec::<u32>();
    /// v.push(1);
    /// v.push(2);
    /// assert_eq!(v.as_slice(), &[1, 2]);
    /// ```
    #[must_use]
    #[inline]
    pub const fn alloc_vec<T>(&self) -> Vec<'_, T, A> {
        Vec::new_in(self)
    }

    /// Create a new growable arena-backed [`Vec`](crate::vec::Vec) with capacity.
    ///
    /// At least `cap` elements of capacity are pre-allocated.
    ///
    /// # Panics
    ///
    /// Panics if the backing allocator fails or if the data alignment is at least 32 KiB.
    /// Use [`Self::try_alloc_vec_with_capacity`] for a fallible variant.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let mut v = arena.alloc_vec_with_capacity::<u32>(100);
    /// for i in 0..50 {
    ///     v.push(i);
    /// }
    /// assert!(v.capacity() >= 100);
    /// ```
    #[must_use]
    #[inline]
    pub fn alloc_vec_with_capacity<T>(&self, cap: usize) -> Vec<'_, T, A> {
        Vec::with_capacity_in(cap, self)
    }

    /// Fallible variant of [`Self::alloc_vec_with_capacity`].
    ///
    /// # Errors
    ///
    /// Returns [`AllocError`] if the backing allocator fails or if the data alignment
    /// is at least 32 KiB.
    ///
    /// # Example
    ///
    /// ```
    /// let arena = multitude::Arena::new();
    /// let Ok(value) = arena.try_alloc_vec_with_capacity::<u32>(8) else {
    ///     panic!("allocation failed");
    /// };
    /// assert!(value.capacity() >= 8);
    /// ```
    #[inline]
    pub fn try_alloc_vec_with_capacity<T>(&self, cap: usize) -> Result<Vec<'_, T, A>, AllocError> {
        Vec::try_with_capacity_in(cap, self)
    }
}
