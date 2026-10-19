// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use performables::arc::{Arc, PerProcess};
use thread_aware_core::ThreadAware;

fn require_thread_aware<T: ThreadAware>() {}

fn main() {
    require_thread_aware::<Arc<u64, PerProcess>>();
}
