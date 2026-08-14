// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Integration tests for explicit allocation-hint backend initialization.

use allocation_hints::domain::Domain;
use allocation_hints::heap::{CreationError, Heap};

rallocator::rallocator!();

#[test]
fn ordinary_allocation_does_not_initialize_the_hint_backend() {
    assert!(matches!(Heap::try_new(), Err(CreationError::BackendUnavailable)));
    assert!(Domain::try_new().is_none());

    let value = Box::new([42_u8; 64]);
    assert_eq!(value[0], 42);

    assert!(matches!(Heap::try_new(), Err(CreationError::BackendUnavailable)));
    assert!(Domain::try_new().is_none());

    rallocator::initialize();

    let heap = Heap::try_new().expect("initialization installs the rallocator heap backend");
    let domain = Domain::try_new().expect("initialization installs the rallocator domain backend");
    drop((heap, domain));
}
