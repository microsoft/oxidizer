// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Conversion of supported inputs to byte views.

use bytesbuf::BytesView;
use compressors::{InputData, Resources};

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
