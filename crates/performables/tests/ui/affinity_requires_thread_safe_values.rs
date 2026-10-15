// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use std::cell::Cell;

use performables::arc::{Arc, PerNuma, PerThread};
use thread_aware_core::ThreadAware;

fn require_thread_aware<T: ThreadAware>() {}

fn main() {
    require_thread_aware::<Arc<Cell<u64>, PerThread>>();
    require_thread_aware::<Arc<Cell<u64>, PerNuma>>();
}
