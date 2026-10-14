// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use performables::sync::mutex::Mutex;

fn main() {
    let mutex = Mutex::<u64>::new(0);
    drop(mutex.lock_async());
}
