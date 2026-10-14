// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Consumer-level checks for synchronization and affinity type-system contracts.

use performables::sync::lock::{RwLockReadGuard, RwLockWriteGuard};
use performables::sync::mutex::MutexGuard;
use static_assertions::assert_not_impl_any;

assert_not_impl_any!(MutexGuard<'static, u64>: Send);
assert_not_impl_any!(RwLockReadGuard<'static, u64>: Send);
assert_not_impl_any!(RwLockWriteGuard<'static, u64>: Send);

#[test]
#[cfg_attr(
    miri,
    ignore = "UI contracts run natively and under cargo careful; they require Cargo and rustc subprocesses"
)]
fn type_system_contracts() {
    let tests = trybuild::TestCases::new();
    tests.pass("tests/ui/pass/*.rs");
    tests.compile_fail("tests/ui/*.rs");
}
