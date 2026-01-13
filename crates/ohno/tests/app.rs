// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! App module tests.

#[cfg(feature = "test-util")]
mod app {
    mod app_err;
    mod bail;
    mod base;
    mod chain;
    mod construction;
    mod conversion;
    mod enrich_err;
    mod into_std_error;
    mod into_trait;
}
