// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Keeps package-scoped test runs non-empty on non-Windows targets.

#![cfg(not(windows))]

#[test]
fn package_has_a_non_windows_test_target() {}
