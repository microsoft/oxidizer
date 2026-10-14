// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

use performables::sync::lock::RwLock;

fn main() {
    let lock = RwLock::<u64>::new(0);
    drop(lock.read_async());
    drop(lock.write_async());
}
