// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use performables::sync::{Sync, condition::Condvar, mutex::Mutex};

fn main() {
    let mutex = Mutex::<u64>::new(0);
    let condition = Condvar::<Sync>::new();
    drop(condition.wait_async(mutex.lock()));
}
