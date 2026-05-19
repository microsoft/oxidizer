// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// Emit the shared `as_str` / `len` / `is_empty` accessor impls for an
/// arena-string smart pointer whose `data: NonNull<u8>` field points at the
/// first data byte (with a `usize` length prefix at `data.sub(8)`).
///
/// `$ty` is the smart pointer type with all of its generics (e.g.
/// `RcStr<A>`); `$($g:tt)+` is the generic-parameter list that
/// goes after `impl` (e.g. `<A: Allocator + Clone>`).
macro_rules! impl_str_accessors {
    ([$($g:tt)+], $ty:ty) => {
        impl $($g)+ $ty {
            /// Borrow as `&str`.
            #[must_use]
            #[inline]
            pub fn as_str(&self) -> &str {
                let data = self.data_ptr();
                // SAFETY: layout invariant — `usize` length prefix
                // immediately before data. The prefix may be
                // unaligned; `read_unaligned` does no alignment check.
                let len = unsafe { ::core::ptr::read_unaligned(data.cast::<usize>().as_ptr().sub(1)) };
                // SAFETY: arena wrote valid UTF-8 bytes of `len` length at `data`.
                unsafe { ::core::str::from_utf8_unchecked(::core::slice::from_raw_parts(data.as_ptr(), len)) }
            }

            /// String length in bytes.
            #[must_use]
            #[inline]
            pub fn len(&self) -> usize {
                // SAFETY: layout invariant — `usize` length prefix
                // immediately before data (possibly unaligned).
                unsafe { ::core::ptr::read_unaligned(self.data_ptr().cast::<usize>().as_ptr().sub(1)) }
            }

            /// True iff the string is empty.
            #[must_use]
            #[inline]
            pub fn is_empty(&self) -> bool {
                self.len() == 0
            }
        }
    };
}

/// Element-generic read-only trait scaffolding shared by both UTF-8 and
/// UTF-16 string handles (`Deref`, `AsRef`, `Borrow`, `Debug`,
/// `Display`, `PartialEq`, `Eq`, `PartialOrd`, `Ord`, `Hash`,
/// `Pointer`). `$view` is the borrowed view type (`str` /
/// `::widestring::Utf16Str`); `$as_view` is the accessor ident.
macro_rules! impl_str_read_traits_generic {
    ([$($g:tt)+], $ty:ty, $view:ty, $as_view:ident) => {
        impl $($g)+ ::core::ops::Deref for $ty {
            type Target = $view;
            #[inline]
            fn deref(&self) -> &$view { self.$as_view() }
        }

        impl $($g)+ ::core::convert::AsRef<$view> for $ty {
            #[inline]
            fn as_ref(&self) -> &$view { self.$as_view() }
        }

        impl $($g)+ ::core::borrow::Borrow<$view> for $ty {
            #[inline]
            fn borrow(&self) -> &$view { self.$as_view() }
        }

        impl $($g)+ ::core::fmt::Debug for $ty {
            #[inline]
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Debug::fmt(self.$as_view(), f)
            }
        }

        impl $($g)+ ::core::fmt::Display for $ty {
            #[inline]
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Display::fmt(self.$as_view(), f)
            }
        }

        impl $($g)+ ::core::cmp::PartialEq for $ty {
            #[inline]
            fn eq(&self, other: &Self) -> bool { self.$as_view() == other.$as_view() }
        }
        impl $($g)+ ::core::cmp::Eq for $ty {}

        impl $($g)+ ::core::cmp::PartialOrd for $ty {
            #[inline]
            fn partial_cmp(&self, other: &Self) -> ::core::option::Option<::core::cmp::Ordering> {
                ::core::option::Option::Some(self.cmp(other))
            }
        }

        impl $($g)+ ::core::cmp::Ord for $ty {
            #[inline]
            fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
                self.$as_view().cmp(other.$as_view())
            }
        }

        impl $($g)+ ::core::hash::Hash for $ty {
            fn hash<H: ::core::hash::Hasher>(&self, state: &mut H) {
                ::core::hash::Hash::hash(self.$as_view(), state)
            }
        }

        impl $($g)+ ::core::fmt::Pointer for $ty {
            #[inline]
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Pointer::fmt(&self.data_ptr().as_ptr(), f)
            }
        }
    };
}

pub(crate) use impl_str_read_traits_generic;

/// Emit the shared read-only trait impls (`Deref`, `AsRef<str>`,
/// `Borrow<str>`, `Debug`, `Display`, `PartialEq`, `Eq`, `PartialOrd`,
/// `Ord`, `Hash`) that delegate to `as_str`.
macro_rules! impl_str_read_traits {
    ([$($g:tt)+], $ty:ty) => {
        $crate::arena_str_macros::impl_str_read_traits_generic!([$($g)+], $ty, str, as_str);
    };
}

pub(crate) use {impl_str_accessors, impl_str_read_traits};

/// Element-generic Local-flavor storage scaffolding (`from_raw_data`,
/// `from_in_chunk`, `from_owned_in_chunk`, `Clone`) shared by both the
/// UTF-8 and UTF-16 string handles. `$elem` is `u8` or `u16`.
macro_rules! impl_str_handle_local_storage {
    ($ty:ident, $elem:ty) => {
        impl<A: ::allocator_api2::alloc::Allocator + ::core::clone::Clone> $ty<A> {
            /// # Safety
            ///
            /// The chunk's refcount must already have been incremented
            /// for this smart pointer, and the buffer at `data` must
            /// hold a properly-formatted arena-resident string buffer
            /// (length prefix immediately before `data`).
            #[inline]
            #[must_use]
            pub(crate) const unsafe fn from_raw_data(data: ::core::ptr::NonNull<$elem>) -> Self {
                // SAFETY: caller forwards the in-chunk invariant.
                unsafe { Self::from_in_chunk($crate::internal::in_chunk::InLocalChunk::new(data)) }
            }

            /// In-chunk-typed counterpart of [`Self::from_raw_data`].
            ///
            /// # Safety
            ///
            /// Same contract as [`Self::from_raw_data`] except the
            /// in-chunk invariant is encoded by the type.
            #[inline]
            #[must_use]
            pub(crate) const unsafe fn from_in_chunk(data: $crate::internal::in_chunk::InLocalChunk<$elem, A>) -> Self {
                Self {
                    data,
                    _phantom: ::core::marker::PhantomData,
                }
            }

            /// Construct from an [`OwnedInLocalChunk`] that already
            /// carries both the in-chunk invariant and the `+1`
            /// refcount ownership. Safe at the call site.
            #[inline]
            #[must_use]
            pub(crate) fn from_owned_in_chunk(owned: $crate::internal::owned_in_chunk::OwnedInLocalChunk<$elem, A>) -> Self {
                Self {
                    data: owned.into_in_chunk(),
                    _phantom: ::core::marker::PhantomData,
                }
            }
        }
        impl<A: ::allocator_api2::alloc::Allocator + ::core::clone::Clone> ::core::clone::Clone for $ty<A> {
            #[inline]
            fn clone(&self) -> Self {
                // SAFETY: this handle holds a +1 on the chunk, so the header is live.
                unsafe { self.data.chunk_ptr().as_ref() }.inc_ref();
                Self {
                    data: self.data,
                    _phantom: ::core::marker::PhantomData,
                }
            }
        }
    };
}

/// Element-generic Shared-flavor counterpart of
/// [`impl_str_handle_local_storage`]. `$elem` is `u8` or `u16`.
macro_rules! impl_str_handle_shared_storage {
    ($ty:ident, $elem:ty) => {
        impl<A: ::allocator_api2::alloc::Allocator + ::core::clone::Clone> $ty<A> {
            /// Construct from an [`OwnedInSharedChunk`] that already
            /// carries both the in-chunk invariant and the `+1`
            /// refcount ownership. Safe at the call site.
            #[inline]
            #[must_use]
            pub(crate) fn from_owned_in_chunk(owned: $crate::internal::owned_in_chunk::OwnedInSharedChunk<$elem, A>) -> Self {
                Self {
                    data: owned.into_in_chunk(),
                    _phantom: ::core::marker::PhantomData,
                }
            }
        }
        impl<A: ::allocator_api2::alloc::Allocator + ::core::clone::Clone> ::core::clone::Clone for $ty<A> {
            #[inline]
            fn clone(&self) -> Self {
                // SAFETY: this handle holds a refcount on the chunk, so the header is live.
                unsafe { self.data.chunk_ptr().as_ref() }.inc_ref();
                Self {
                    data: self.data,
                    _phantom: ::core::marker::PhantomData,
                }
            }
        }
    };
}

/// Element-generic `@body` scaffolding shared by both
/// `impl_str_handle_core` (UTF-8) and `impl_utf16_str_handle_core`
/// (UTF-16). Emits `data_ptr`, `Unpin`, `PartialEq<$view>` /
/// `PartialEq<&$view>`. Each caller emits its own `Serialize` impl
/// (UTF-8 hands the bytes directly to `serialize_str`; UTF-16
/// transcodes via `to_string`).
macro_rules! impl_str_handle_body_generic {
    ($ty:ident, $elem:ty, $view:ty, $as_view:ident) => {
        impl<A: ::allocator_api2::alloc::Allocator + ::core::clone::Clone> $ty<A> {
            /// Pointer to the first data element; the inline length prefix lives at
            /// `data_ptr().cast::<usize>().sub(1)` (possibly unaligned).
            #[inline]
            pub(crate) const fn data_ptr(&self) -> ::core::ptr::NonNull<$elem> {
                self.data.as_non_null()
            }
        }

        // Mirrors the underlying smart-pointer flavor's `Unpin` impl.
        impl<A: ::allocator_api2::alloc::Allocator + ::core::clone::Clone> ::core::marker::Unpin for $ty<A> {}

        impl<A: ::allocator_api2::alloc::Allocator + ::core::clone::Clone> ::core::cmp::PartialEq<$view> for $ty<A> {
            #[inline]
            fn eq(&self, other: &$view) -> bool {
                self.$as_view() == other
            }
        }

        impl<A: ::allocator_api2::alloc::Allocator + ::core::clone::Clone> ::core::cmp::PartialEq<&$view> for $ty<A> {
            #[inline]
            fn eq(&self, other: &&$view) -> bool {
                self.$as_view() == *other
            }
        }
    };
}

pub(crate) use impl_str_handle_body_generic;

/// Emit the boilerplate handle scaffolding (`Clone`, `from_raw_data`,
/// `data_ptr`, `Unpin`, `PartialEq<str>` / `PartialEq<&str>`, optional
/// `serde::Serialize`) for a UTF-8 arena-string smart pointer whose
/// `inner: RawHandle<u8, $flavor, A>` field holds the chunk-refcounted
/// pointer to the first data byte.
///
/// This is the cross-flavor twin: `RcStr` and `ArcStr` both expand to
/// the same body, parameterized only by the flavor type.
macro_rules! impl_str_handle_core {
    (@body $ty:ident) => {
        $crate::arena_str_macros::impl_str_handle_body_generic!($ty, u8, str, as_str);

        #[cfg(feature = "serde")]
        #[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
        impl<A: ::allocator_api2::alloc::Allocator + ::core::clone::Clone> ::serde::ser::Serialize for $ty<A> {
            fn serialize<S: ::serde::ser::Serializer>(&self, serializer: S) -> ::core::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }
    };

    ($ty:ident, Local) => {
        $crate::arena_str_macros::impl_str_handle_local_storage!($ty, u8);
        impl_str_handle_core!(@body $ty);
    };

    ($ty:ident, Shared) => {
        $crate::arena_str_macros::impl_str_handle_shared_storage!($ty, u8);
        impl_str_handle_core!(@body $ty);
    };
}

pub(crate) use {impl_str_handle_core, impl_str_handle_local_storage, impl_str_handle_shared_storage};

/// UTF-16 sibling of [`impl_str_handle_core`] — emits `Clone`,
/// `from_raw_data`, `data_ptr`, `Unpin`, `PartialEq<Utf16Str>` /
/// `PartialEq<&Utf16Str>`, and (when the `serde` feature is enabled)
/// `Serialize` for a UTF-16 string smart pointer whose
/// `inner: RawHandle<u16, $flavor, A>` holds the chunk-refcounted
/// pointer to the first `u16` element.
#[cfg(feature = "utf16")]
macro_rules! impl_utf16_str_handle_core {
    (@body $ty:ident) => {
        $crate::arena_str_macros::impl_str_handle_body_generic!($ty, u16, ::widestring::Utf16Str, as_utf16_str);

        #[cfg(feature = "serde")]
        #[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
        impl<A: ::allocator_api2::alloc::Allocator + ::core::clone::Clone> ::serde::ser::Serialize for $ty<A> {
            fn serialize<S: ::serde::ser::Serializer>(&self, serializer: S) -> ::core::result::Result<S::Ok, S::Error> {
                // Transcode UTF-16 → UTF-8 on the wire.
                let owned: ::alloc::string::String = self.as_utf16_str().to_string();
                serializer.serialize_str(&owned)
            }
        }
    };

    ($ty:ident, Local) => {
        $crate::arena_str_macros::impl_str_handle_local_storage!($ty, u16);
        impl_utf16_str_handle_core!(@body $ty);
    };

    ($ty:ident, Shared) => {
        $crate::arena_str_macros::impl_str_handle_shared_storage!($ty, u16);
        impl_utf16_str_handle_core!(@body $ty);
    };
}

#[cfg(feature = "utf16")]
pub(crate) use impl_utf16_str_handle_core;

/// Emit the shared `as_utf16_str` / `len` / `is_empty` accessor impls
/// for an arena UTF-16 string smart pointer whose `data: NonNull<u16>`
/// field points at the first `u16` element (with a `usize` element-count
/// prefix at `data.cast::<usize>().sub(1)`).
#[cfg(feature = "utf16")]
macro_rules! impl_utf16_str_accessors {
    ([$($g:tt)+], $ty:ty) => {
        impl $($g)+ $ty {
            /// Borrow as `&Utf16Str`.
            #[must_use]
            #[inline]
            pub fn as_utf16_str(&self) -> &::widestring::Utf16Str {
                let data = self.data_ptr();
                // SAFETY: layout invariant — `usize` element-count
                // prefix immediately before data (possibly unaligned).
                let len = unsafe { ::core::ptr::read_unaligned(data.cast::<usize>().as_ptr().sub(1)) };
                // SAFETY: arena wrote `len` valid `u16` elements at `data`.
                let slice = unsafe { ::core::slice::from_raw_parts(data.as_ptr(), len) };
                // SAFETY: arena guarantees the slice is valid UTF-16.
                unsafe { ::widestring::Utf16Str::from_slice_unchecked(slice) }
            }

            /// String length in `u16` elements.
            #[must_use]
            #[inline]
            pub fn len(&self) -> usize {
                // SAFETY: layout invariant; prefix may be unaligned.
                unsafe { ::core::ptr::read_unaligned(self.data_ptr().cast::<usize>().as_ptr().sub(1)) }
            }

            /// True iff the string is empty.
            #[must_use]
            #[inline]
            pub fn is_empty(&self) -> bool {
                self.len() == 0
            }
        }
    };
}

/// Emit the shared read-only trait impls (`Deref`, `AsRef<Utf16Str>`,
/// `Borrow<Utf16Str>`, `Debug`, `Display`, `PartialEq`, `Eq`, `PartialOrd`,
/// `Ord`, `Hash`, `Pointer`) that delegate to `as_utf16_str`.
#[cfg(feature = "utf16")]
macro_rules! impl_utf16_str_read_traits {
    ([$($g:tt)+], $ty:ty) => {
        $crate::arena_str_macros::impl_str_read_traits_generic!(
            [$($g)+], $ty, ::widestring::Utf16Str, as_utf16_str
        );
    };
}

#[cfg(feature = "utf16")]
pub(crate) use impl_utf16_str_accessors;
#[cfg(feature = "utf16")]
pub(crate) use impl_utf16_str_read_traits;

/// Emits the shared trait/Drop boilerplate for the mutable arena string
/// builders (`String` and `Utf16String`). Both types have the shape
/// `Ty<'a, A: Allocator + Clone>` and store `data: NonNull<u8>` with a
/// `cap` field; `cap > 0` indicates an outstanding chunk refcount that
/// must be released on drop.
///
/// Arguments:
/// - `$Ty`        : the type ident (e.g. `String`, `Utf16String`)
/// - `$lt`        : the lifetime label (canonically `'a`)
/// - `$StrTy`     : the borrowed string slice type (`str` / `::widestring::Utf16Str`)
/// - `$as_str`    : the immutable accessor ident (`as_str` / `as_utf16_str`)
/// - `$as_mut`    : the mutable accessor ident (`as_mut_str` / `as_mut_utf16_str`)
macro_rules! impl_arena_string_forwarding_traits {
    ($Ty:ident, $lt:lifetime, $StrTy:ty, $as_str:ident, $as_mut:ident) => {
        impl<A: Allocator + Clone> Drop for $Ty<'_, A> {
            #[inline]
            fn drop(&mut self) {
                if self.cap > 0 {
                    // SAFETY: cap > 0 ⇒ `data` points into a live local chunk
                    // that this builder owns +1 on.
                    let in_chunk = unsafe { crate::internal::in_chunk::InLocalChunk::<_, A>::new(self.data) };
                    let chunk = in_chunk.chunk_ptr();
                    // SAFETY: this builder owns one positive chunk refcount.
                    unsafe { crate::internal::local_chunk::LocalChunk::dec_ref(chunk) };
                }
            }
        }

        impl<A: Allocator + Clone> ::core::ops::Deref for $Ty<'_, A> {
            type Target = $StrTy;
            #[inline]
            fn deref(&self) -> &$StrTy {
                self.$as_str()
            }
        }

        impl<A: Allocator + Clone> ::core::ops::DerefMut for $Ty<'_, A> {
            #[inline]
            fn deref_mut(&mut self) -> &mut $StrTy {
                self.$as_mut()
            }
        }

        impl<A: Allocator + Clone> ::core::convert::AsRef<$StrTy> for $Ty<'_, A> {
            fn as_ref(&self) -> &$StrTy {
                self.$as_str()
            }
        }

        impl<A: Allocator + Clone> ::core::convert::AsMut<$StrTy> for $Ty<'_, A> {
            fn as_mut(&mut self) -> &mut $StrTy {
                self.$as_mut()
            }
        }

        impl<A: Allocator + Clone> ::core::borrow::Borrow<$StrTy> for $Ty<'_, A> {
            fn borrow(&self) -> &$StrTy {
                self.$as_str()
            }
        }

        impl<A: Allocator + Clone> ::core::borrow::BorrowMut<$StrTy> for $Ty<'_, A> {
            fn borrow_mut(&mut self) -> &mut $StrTy {
                self.$as_mut()
            }
        }

        impl<A: Allocator + Clone> ::core::fmt::Debug for $Ty<'_, A> {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Debug::fmt(self.$as_str(), f)
            }
        }

        impl<A: Allocator + Clone> ::core::fmt::Display for $Ty<'_, A> {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                ::core::fmt::Display::fmt(self.$as_str(), f)
            }
        }

        impl<A: Allocator + Clone> ::core::cmp::PartialEq for $Ty<'_, A> {
            fn eq(&self, other: &Self) -> bool {
                self.$as_str() == other.$as_str()
            }
        }

        impl<A: Allocator + Clone> ::core::cmp::Eq for $Ty<'_, A> {}

        impl<A: Allocator + Clone> ::core::cmp::PartialOrd for $Ty<'_, A> {
            fn partial_cmp(&self, other: &Self) -> Option<::core::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        impl<A: Allocator + Clone> ::core::cmp::Ord for $Ty<'_, A> {
            fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
                self.$as_str().cmp(other.$as_str())
            }
        }

        impl<A: Allocator + Clone> ::core::hash::Hash for $Ty<'_, A> {
            fn hash<H: ::core::hash::Hasher>(&self, state: &mut H) {
                ::core::hash::Hash::hash(self.$as_str(), state);
            }
        }

        impl<A: Allocator + Clone> ::core::cmp::PartialEq<$StrTy> for $Ty<'_, A> {
            #[inline]
            fn eq(&self, other: &$StrTy) -> bool {
                self.$as_str() == other
            }
        }

        impl<A: Allocator + Clone> ::core::cmp::PartialEq<&$StrTy> for $Ty<'_, A> {
            #[inline]
            fn eq(&self, other: &&$StrTy) -> bool {
                self.$as_str() == *other
            }
        }
    };
}

pub(crate) use impl_arena_string_forwarding_traits;

/// Emits the BoxStr-specific extras: `as_mut_str` / `into_rc_str` plus
/// `Drop`, `DerefMut`, `AsMut`, `BorrowMut`, `From<BoxStr> for RcStr`,
/// `PartialEq<str>` / `PartialEq<&str>` and (gated on `serde`) a
/// `Serialize` impl.
/// Element-generic forwarding-trait + Drop scaffolding shared between
/// `impl_box_str_extras` (UTF-8) and `impl_box_utf16_str_extras`
/// (UTF-16). Emits `Drop`, `DerefMut`, `AsMut<$view>`,
/// `BorrowMut<$view>`, `From<$Box<A>> for $Rc<A>`, `PartialEq<$view>`,
/// `PartialEq<&$view>`. Each invoking macro is still responsible for
/// emitting its bespoke `as_mut_*` and `into_rc_*` methods (their
/// element-type-specific bodies differ).
macro_rules! impl_box_str_extras_common {
    ($Box:ident, $Rc:ident, $view:ty, $as_view:ident, $as_mut:ident, $into_rc:ident) => {
        impl<A: Allocator + Clone> Drop for $Box<A> {
            #[inline]
            fn drop(&mut self) {
                let chunk = self.data.chunk_ptr();
                // SAFETY: this handle owns +1 on the chunk.
                unsafe { crate::internal::local_chunk::LocalChunk::dec_ref(chunk) };
            }
        }

        impl<A: Allocator + Clone> ::core::ops::DerefMut for $Box<A> {
            #[inline]
            fn deref_mut(&mut self) -> &mut $view {
                self.$as_mut()
            }
        }

        impl<A: Allocator + Clone> ::core::convert::AsMut<$view> for $Box<A> {
            #[inline]
            fn as_mut(&mut self) -> &mut $view {
                self.$as_mut()
            }
        }

        impl<A: Allocator + Clone> ::core::borrow::BorrowMut<$view> for $Box<A> {
            #[inline]
            fn borrow_mut(&mut self) -> &mut $view {
                self.$as_mut()
            }
        }

        impl<A: Allocator + Clone> From<$Box<A>> for $Rc<A> {
            #[inline]
            fn from(b: $Box<A>) -> Self {
                b.$into_rc()
            }
        }

        impl<A: Allocator + Clone> ::core::cmp::PartialEq<$view> for $Box<A> {
            #[inline]
            fn eq(&self, other: &$view) -> bool {
                self.$as_view() == other
            }
        }

        impl<A: Allocator + Clone> ::core::cmp::PartialEq<&$view> for $Box<A> {
            #[inline]
            fn eq(&self, other: &&$view) -> bool {
                self.$as_view() == *other
            }
        }
    };
}

pub(crate) use impl_box_str_extras_common;

/// Emits the BoxStr-specific extras: `as_mut_str` / `into_rc_str` plus
/// `Drop`, `DerefMut`, `AsMut`, `BorrowMut`, `From<BoxStr> for RcStr`,
/// `PartialEq<str>` / `PartialEq<&str>` and (gated on `serde`) a
/// `Serialize` impl.
macro_rules! impl_box_str_extras {
    ($Box:ident, $Rc:ident) => {
        impl<A: Allocator + Clone> $Box<A> {
            /// Borrow as `&mut str`.
            #[must_use]
            #[inline]
            #[allow(
                clippy::needless_pass_by_ref_mut,
                reason = "&mut self is the API contract — the returned &mut str borrows exclusively from self"
            )]
            pub fn as_mut_str(&mut self) -> &mut str {
                let data = self.data.as_non_null();
                // SAFETY: layout invariant — `usize` length prefix
                // immediately before data (possibly unaligned).
                let len = unsafe { ::core::ptr::read_unaligned(data.cast::<usize>().as_ptr().sub(1)) };
                // SAFETY: arena wrote valid UTF-8 bytes of `len` length at `data`.
                unsafe { core::str::from_utf8_unchecked_mut(core::slice::from_raw_parts_mut(data.as_ptr(), len)) }
            }

            /// Convert this owned, mutable string into a shared, immutable
            /// [`RcStr<A>`](crate::strings::RcStr). O(1) — no copy, no
            /// allocation.
            ///
            /// # Example
            ///
            /// ```
            /// let arena = multitude::Arena::new();
            /// let mut b = arena.alloc_str_box("hello");
            /// b.make_ascii_uppercase();
            /// let s = b.into_rc_str();
            /// let s2 = s.clone();
            /// assert_eq!(&*s, "HELLO");
            /// assert_eq!(&*s, &*s2);
            /// ```
            #[must_use]
            #[inline]
            pub fn into_rc_str(self) -> $Rc<A> {
                let data = self.data.as_non_null();
                core::mem::forget(self);
                // SAFETY: same chunk, same +1 refcount, transfers ownership.
                unsafe { $Rc::from_raw_data(data) }
            }
        }

        $crate::arena_str_macros::impl_box_str_extras_common!($Box, $Rc, str, as_str, as_mut_str, into_rc_str);

        #[cfg(feature = "serde")]
        #[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
        impl<A: Allocator + Clone> serde::ser::Serialize for $Box<A> {
            fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }
    };
}

pub(crate) use impl_box_str_extras;

/// Emits the `BoxUtf16Str`-specific extras: `as_mut_utf16_str` /
/// `into_rc_utf16_str` plus `Drop`, `DerefMut`, `AsMut`, `BorrowMut`,
/// `From`, `PartialEq` and (gated) `Serialize`.
#[cfg(feature = "utf16")]
macro_rules! impl_box_utf16_str_extras {
    ($Box:ident, $Rc:ident) => {
        impl<A: Allocator + Clone> $Box<A> {
            /// Borrow as `&mut Utf16Str`.
            #[must_use]
            #[inline]
            #[allow(
                clippy::needless_pass_by_ref_mut,
                reason = "&mut self is the API contract — the returned &mut Utf16Str borrows exclusively from self"
            )]
            pub fn as_mut_utf16_str(&mut self) -> &mut ::widestring::Utf16Str {
                let data = self.data.as_non_null();
                let len = self.len();
                // SAFETY: `data` points at `len` valid `u16` elements and `&mut self` is exclusive.
                let slice = unsafe { core::slice::from_raw_parts_mut(data.as_ptr(), len) };
                // SAFETY: arena guarantees the slice is valid UTF-16.
                unsafe { ::widestring::Utf16Str::from_slice_unchecked_mut(slice) }
            }

            /// Convert this owned, mutable string into a shared, immutable
            #[doc = concat!("[`", stringify!($Rc), "<A>`]. O(1).")]
            #[must_use]
            #[inline]
            pub fn into_rc_utf16_str(self) -> $Rc<A> {
                let data = self.data.as_non_null();
                core::mem::forget(self);
                // SAFETY: data points at valid UTF-16 data; the +1 refcount transfers from the box.
                unsafe { $Rc::from_raw_data(data) }
            }
        }

        impl<A: Allocator + Clone> ::core::marker::Unpin for $Box<A> {}

        $crate::arena_str_macros::impl_box_str_extras_common!(
            $Box,
            $Rc,
            ::widestring::Utf16Str,
            as_utf16_str,
            as_mut_utf16_str,
            into_rc_utf16_str
        );

        #[cfg(feature = "serde")]
        #[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
        impl<A: Allocator + Clone> serde::ser::Serialize for $Box<A> {
            fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let s = self.as_utf16_str();
                let owned = s.to_string();
                serializer.serialize_str(&owned)
            }
        }
    };
}

#[cfg(feature = "utf16")]
pub(crate) use impl_box_utf16_str_extras;

/// Emits `impl From<$Handle<A>> for crate::$Ptr<[$ElemTy], A>` — an
/// O(1) move that transfers the chunk's +1 refcount to a thin
/// byte/element slice handle.
///
/// `$Handle` is `RcStr` / `ArcStr` / `RcUtf16Str` / `ArcUtf16Str`;
/// `$Ptr` is the matching smart-pointer alias (`Rc` or `Arc`);
/// `$ElemTy` is `u8` or `u16`.
macro_rules! impl_from_str_for_slice_handle {
    ($Handle:ident, $Ptr:ident, $ElemTy:ty) => {
        impl<A: Allocator + Clone> ::core::convert::From<$Handle<A>> for crate::$Ptr<[$ElemTy], A> {
            #[doc = concat!(
                                                "Convert a [`", stringify!($Handle), "<A>`] into an [`",
                                                stringify!($Ptr), "<[", stringify!($ElemTy), "], A>`](crate::", stringify!($Ptr), ")."
                                            )]
            ///
            /// O(1) — the chunk's +1 refcount transfers; the new handle's
            /// slice covers the underlying elements (excluding the inline
            /// length prefix).
            #[inline]
            fn from(s: $Handle<A>) -> Self {
                let data = s.data_ptr();
                let len = s.len();
                core::mem::forget(s);
                let slice_ptr = core::ptr::slice_from_raw_parts_mut(data.as_ptr(), len);
                // SAFETY: `data` points at `len` valid elements; the +1 refcount transfers.
                unsafe { Self::from_value_ptr(core::ptr::NonNull::new_unchecked(slice_ptr)) }
            }
        }
    };
}

pub(crate) use impl_from_str_for_slice_handle;
