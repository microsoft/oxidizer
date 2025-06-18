// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

/// A macro to create a `NonZero` constant from a literal value.
macro_rules! nz {
    ($x:literal) => {
        const { ::std::num::NonZero::new($x).expect("literal must have non-zero value") }
    };
}

pub(crate) use nz;