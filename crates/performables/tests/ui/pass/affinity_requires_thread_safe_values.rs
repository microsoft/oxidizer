// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;

use performables::arc::{Arc, PerNuma, PerThread};
use static_assertions::assert_not_impl_any;
use thread_aware_core::ThreadAware;

assert_not_impl_any!(Arc<Cell<u64>, PerThread>: ThreadAware);
assert_not_impl_any!(Arc<Cell<u64>, PerNuma>: ThreadAware);

fn main() {}
