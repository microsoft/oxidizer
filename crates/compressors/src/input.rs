// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Input types for whole-buffer functions.
//!
//! [`InputData`] accepts an existing [`BytesView`] or a byte slice. Whole-buffer functions use their
//! [`Resources`] to build a view for a slice.

use bytesbuf::BytesView;

use crate::resources::Resources;

pub(crate) mod sealed {
    /// Restricts [`InputData`][super::InputData] to the inputs this crate defines.
    ///
    /// A downstream crate cannot name this `pub(crate)` module. Therefore, it cannot implement the
    /// trait.
    pub trait Sealed {}
}

/// A byte sequence one of this crate's whole-buffer conveniences can compress or decompress.
///
/// Implemented for [`BytesView`], byte slices, and byte arrays. An existing view passes through
/// unchanged. Slices and arrays are copied into memory from the supplied [`Resources`].
///
/// This trait is sealed. Only this crate can add implementations.
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
/// // Copy a slice into memory from the resources.
/// let from_slice = gzip::compress(b"hello", resources)?;
///
/// // Pass an existing view without a copy.
/// let view = BytesView::copied_from_slice(b"hello", resources.memory());
/// let from_view = gzip::compress(view, resources)?;
///
/// assert_eq!(from_slice.to_vec(), from_view.to_vec());
/// # }
/// # Ok::<(), compressors::Error>(())
/// ```
pub trait InputData: sealed::Sealed {
    /// Produces the [`BytesView`] that the engine consumes.
    ///
    /// An existing view passes through without a copy. Other input uses `resources`.
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

// A byte-string literal is `&[u8; N]`. Generic argument matching does not coerce it to `&[u8]`.
// The array implementation lets callers pass `b"..."` without `.as_slice()`.
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
        // `b"..."` is `&[u8; N]`, which is the common call-site type.
        let resources = Resources::default();

        assert_eq!(b"a literal".into_view(&resources).to_vec(), b"a literal".to_vec());
    }
}
