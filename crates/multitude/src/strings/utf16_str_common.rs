// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Shared inherent methods and trait impls for the two single-pointer
//! UTF-16 string types ([`ArcUtf16Str`](super::ArcUtf16Str) and
//! [`BoxUtf16Str`](super::BoxUtf16Str)).
//!
//! Both types share identical layout (`NonNull<u16>` + `PhantomData`),
//! prefix-length reading, payload borrowing, and
//! formatting/comparison/hash/`Pointer`/`Serialize` impls. The macro
//! below emits all of that for a given struct name; per-file blocks
//! supply the items that legitimately differ (`Send`/`Sync` bounds,
//! `Clone` and `Drop` for `Arc`, `Drop` for `Box`, `DerefMut`/`AsMut`/
//! `BorrowMut` and `as_mut_utf16_str` for `Box`).

/// Emit shared inherent shape + trait impls for a single-pointer
/// UTF-16 string type with field layout `{ ptr: NonNull<u16>, _phantom }`.
macro_rules! impl_utf16_str_common {
    ($Ty:ident) => {
        impl<A: allocator_api2::alloc::Allocator + Clone> $Ty<A> {
            /// Reads the inline element-count prefix (unaligned).
            #[inline]
            fn u16_len(&self) -> usize {
                // SAFETY: `ptr` was produced by `impl_alloc_prefixed_shared`,
                // which wrote the u16 element count into the `usize`
                // immediately preceding the payload.
                unsafe { $crate::arena::alloc_prefixed::read_prefix_len(self.ptr) }
            }

            /// Borrow as `&Utf16Str`.
            #[must_use]
            #[inline]
            pub fn as_utf16_str(&self) -> &widestring::Utf16Str {
                let len = self.u16_len();
                // SAFETY: `ptr` addresses `len` initialized, valid UTF-16
                // code units in a chunk kept alive by `self`.
                unsafe {
                    let units = core::slice::from_raw_parts(self.ptr.as_ptr(), len);
                    widestring::Utf16Str::from_slice_unchecked(units)
                }
            }

            /// String length in `u16` elements.
            #[must_use]
            #[inline]
            pub fn len(&self) -> usize {
                self.u16_len()
            }

            /// True iff the string is empty.
            #[must_use]
            #[inline]
            pub fn is_empty(&self) -> bool {
                self.u16_len() == 0
            }
        }

        impl<A: allocator_api2::alloc::Allocator + Clone> Unpin for $Ty<A> {}

        impl<A: allocator_api2::alloc::Allocator + Clone> core::ops::Deref for $Ty<A> {
            type Target = widestring::Utf16Str;
            #[inline]
            fn deref(&self) -> &widestring::Utf16Str {
                self.as_utf16_str()
            }
        }

        impl<A: allocator_api2::alloc::Allocator + Clone> AsRef<widestring::Utf16Str> for $Ty<A> {
            #[inline]
            fn as_ref(&self) -> &widestring::Utf16Str {
                self.as_utf16_str()
            }
        }

        impl<A: allocator_api2::alloc::Allocator + Clone> core::borrow::Borrow<widestring::Utf16Str> for $Ty<A> {
            #[inline]
            fn borrow(&self) -> &widestring::Utf16Str {
                self.as_utf16_str()
            }
        }

        impl<A: allocator_api2::alloc::Allocator + Clone> core::fmt::Debug for $Ty<A> {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Debug::fmt(self.as_utf16_str(), f)
            }
        }

        impl<A: allocator_api2::alloc::Allocator + Clone> core::fmt::Display for $Ty<A> {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Display::fmt(self.as_utf16_str(), f)
            }
        }

        impl<A: allocator_api2::alloc::Allocator + Clone> PartialEq for $Ty<A> {
            #[inline]
            fn eq(&self, other: &Self) -> bool {
                self.as_utf16_str() == other.as_utf16_str()
            }
        }
        impl<A: allocator_api2::alloc::Allocator + Clone> Eq for $Ty<A> {}

        impl<A: allocator_api2::alloc::Allocator + Clone> PartialOrd for $Ty<A> {
            #[inline]
            fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
                Some(self.cmp(other))
            }
        }

        impl<A: allocator_api2::alloc::Allocator + Clone> Ord for $Ty<A> {
            #[inline]
            fn cmp(&self, other: &Self) -> core::cmp::Ordering {
                self.as_utf16_str().cmp(other.as_utf16_str())
            }
        }

        impl<A: allocator_api2::alloc::Allocator + Clone> core::hash::Hash for $Ty<A> {
            fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
                self.as_utf16_str().hash(state);
            }
        }

        impl<A: allocator_api2::alloc::Allocator + Clone> core::fmt::Pointer for $Ty<A> {
            #[inline]
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                core::fmt::Pointer::fmt(&self.ptr.as_ptr(), f)
            }
        }

        impl<A: allocator_api2::alloc::Allocator + Clone> PartialEq<widestring::Utf16Str> for $Ty<A> {
            #[inline]
            fn eq(&self, other: &widestring::Utf16Str) -> bool {
                self.as_utf16_str() == other
            }
        }

        impl<A: allocator_api2::alloc::Allocator + Clone> PartialEq<&widestring::Utf16Str> for $Ty<A> {
            #[inline]
            fn eq(&self, other: &&widestring::Utf16Str) -> bool {
                self.as_utf16_str() == *other
            }
        }

        #[cfg(feature = "serde")]
        #[cfg_attr(docsrs, doc(cfg(feature = "serde")))]
        impl<A: allocator_api2::alloc::Allocator + Clone> serde::ser::Serialize for $Ty<A> {
            fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self.as_utf16_str())
            }
        }
    };
}

pub(crate) use impl_utf16_str_common;
