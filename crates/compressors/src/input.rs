// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! What the whole-buffer conveniences accept as input.
//!
//! [`InputData`] is why `gzip::compress(b"hello", resources)` reads as well as
//! `gzip::compress(view, resources)`: the conveniences already hold the [`Resources`] a
//! [`BytesView`] needs to be built from, so a caller with a plain slice does not have to build one
//! by hand first.

use bytesbuf::BytesView;

use crate::resources::Resources;

pub(crate) mod sealed {
    /// Restricts [`InputData`][super::InputData] to the inputs this crate defines.
    ///
    /// Living in a `pub(crate)` module makes it unnameable downstream, so the trait cannot be
    /// implemented outside this crate even though its own method is public.
    pub trait Sealed {}
}

/// A byte sequence one of this crate's whole-buffer conveniences can compress or decompress.
///
/// Implemented for [`BytesView`], which is passed through untouched, and for byte slices and
/// arrays, which are copied into memory drawn from the [`Resources`] the convenience was given.
///
/// The trait is sealed: it names the inputs this crate accepts rather than being an extension
/// point, so implementations can be added without that being a breaking change.
///
/// # Examples
///
/// ```
/// # #[cfg(feature = "gzip")]
/// # {
/// use bytesbuf::BytesView;
/// use compressors::{Resources, gzip};
///
/// let resources = Resources::global();
///
/// // A slice, copied into the resources' memory on the way in.
/// let from_slice = gzip::compress(b"hello", resources)?;
///
/// // A view that already exists, used as it is.
/// let view = BytesView::copied_from_slice(b"hello", resources.memory());
/// let from_view = gzip::compress(view, resources)?;
///
/// assert_eq!(from_slice.to_vec(), from_view.to_vec());
/// # }
/// # Ok::<(), compressors::Error>(())
/// ```
pub trait InputData: sealed::Sealed {
    /// Produces the [`BytesView`] the codec consumes, allocating from `resources` if it has to.
    ///
    /// Taking `self` by value is what lets an existing view be forwarded without a copy.
    fn into_view(self, resources: &Resources) -> BytesView;
}

impl sealed::Sealed for BytesView {}

impl InputData for BytesView {
    fn into_view(self, _resources: &Resources) -> BytesView {
        self
    }
}

impl sealed::Sealed for &[u8] {}

impl InputData for &[u8] {
    fn into_view(self, resources: &Resources) -> BytesView {
        BytesView::copied_from_slice(self, resources.memory())
    }
}

// A byte-string literal is `&[u8; N]`, which does not coerce to `&[u8]` while matching a generic
// parameter, so the array carries its own implementation rather than making every caller write
// `b"..."` followed by `.as_slice()`.
impl<const N: usize> sealed::Sealed for &[u8; N] {}

impl<const N: usize> InputData for &[u8; N] {
    fn into_view(self, resources: &Resources) -> BytesView {
        BytesView::copied_from_slice(self.as_slice(), resources.memory())
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_is_passed_through_unchanged() {
        let resources = Resources::default();
        let view = BytesView::copied_from_slice(b"already a view", resources.memory());

        assert_eq!(view.clone().into_view(&resources).to_vec(), view.to_vec());
    }

    #[test]
    fn a_slice_is_copied_into_the_supplied_memory() {
        let resources = Resources::default();
        let slice: &[u8] = b"a plain slice";

        assert_eq!(slice.into_view(&resources).to_vec(), slice.to_vec());
    }

    #[test]
    fn an_array_reference_is_copied_like_a_slice() {
        // `b"..."` is `&[u8; N]`, so this is the shape most call sites actually have.
        let resources = Resources::default();

        assert_eq!(b"a literal".into_view(&resources).to_vec(), b"a literal".to_vec());
    }
}
